//! The gRPC transport adapter for the [`Engine`] service: a driving adapter
//! that implements `engine_entities`' generated tonic service trait against
//! `core_entities::ports` alone - [`core_entities::ports::catalog`]'s
//! [`ServiceCatalog`]/[`ServiceCatalogWriter`] and
//! [`core_entities::ports::engine`]'s [`EngineService`] - never against a
//! concrete storage, writer, or execution-engine crate. A composition root
//! (e.g. `apid`) bootstraps the concrete adapters behind those ports and
//! calls [`serve`] to start listening; this crate has no other public
//! surface.

use std::{
    collections::HashMap,
    net::SocketAddr,
    panic,
    sync::{Arc, Mutex, PoisonError},
};

use core_entities::ports::{
    catalog::{error::CatalogError, ServiceCatalog, ServiceCatalogWriter},
    engine::{self, EngineInputContext, EngineService},
};
use core_entities::service::{service_manifest_latest, VersionedServiceTree};
use core_json_compat::{from_json, to_json};
use credential_entities::credentials::Authentication;
use engine_entities::engine::{
    engine_server::{Engine, EngineServer},
    get_run_result_response,
    list_response::ListItem,
    GetRunResultRequest, GetRunResultResponse, GetSerivceRequest, GetServiceResponse, ListRequest,
    ListResponse, RunServiceRequest, RunServiceResponse, SaveServiceRequest, SaveServiceResponse,
};
use tonic::{transport::Server, Request, Response, Status};

/// Implements the gRPC [`Engine`] service over a loaded-service catalog and
/// an execution engine, reached only through their ports.
pub struct EngineGrpcService {
    /// Read access to the loaded service catalog.
    catalog: Arc<dyn ServiceCatalog + Send + Sync>,

    /// Write access to the loaded service catalog.
    catalog_writer: Arc<dyn ServiceCatalogWriter + Send + Sync>,

    /// The execution engine runs are dispatched to.
    engine: Arc<dyn EngineService>,

    /// Results of in-flight and completed runs, keyed by execution ID.
    responses: Arc<Mutex<HashMap<String, GetRunResultResponse>>>,
}

impl EngineGrpcService {
    /// Builds an [`EngineGrpcService`] over `catalog`, `catalog_writer`, and
    /// `engine`. The in-flight run-result cache is internal state, not a
    /// caller-supplied dependency.
    #[must_use]
    #[inline]
    pub fn new(
        catalog: Arc<dyn ServiceCatalog + Send + Sync>,
        catalog_writer: Arc<dyn ServiceCatalogWriter + Send + Sync>,
        engine: Arc<dyn EngineService>,
    ) -> Self {
        Self {
            catalog,
            catalog_writer,
            engine,
            responses: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[tonic::async_trait]
impl Engine for EngineGrpcService {
    async fn list(&self, _: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        let mut items = vec![];

        for id in self.catalog.list() {
            if let Some(service) = self.catalog.get_service(&id) {
                let v1 = service.v1();
                let manifest = v1.manifest_latest();

                match &manifest.value {
                    Some(service_manifest_latest::Value::Swagger(_)) => {
                        let operations = v1
                            .common_api
                            .as_ref()
                            .map(|api| api.operations.keys())
                            .into_iter()
                            .flatten();
                        for op_name in operations {
                            items.push(ListItem {
                                name: format!("(swagger) {id}.{op_name}"),
                            });
                        }
                    }
                    Some(service_manifest_latest::Value::Action(action)) => {
                        for op in &action.operations {
                            items.push(ListItem {
                                name: format!("(action) {id}.{}", op.id),
                            });
                        }
                    }
                    Some(service_manifest_latest::Value::ApiWrapped(_)) => {
                        items.push(ListItem {
                            name: format!("(wrapped) {id}.execute"),
                        });
                    }
                    Some(service_manifest_latest::Value::SimpleCode(_)) => {
                        items.push(ListItem {
                            name: format!("(code) {id}.execute"),
                        });
                    }
                    Some(
                        service_manifest_latest::Value::ScriptedAction(_)
                        | service_manifest_latest::Value::Workflow(_),
                    )
                    | None => {}
                }
            }
            // Else log
        }

        let response = ListResponse { items };

        Ok(Response::new(response))
    }

    async fn get_service(
        &self,
        req: Request<GetSerivceRequest>,
    ) -> Result<Response<GetServiceResponse>, Status> {
        let req = req.into_inner();

        let service = self
            .catalog
            .get_service(&req.name)
            .ok_or_else(|| Status::not_found("Service not found"))?;
        let credentials = self.catalog.get_credentials(&req.name);

        let raw_service =
            serde_json::to_vec(&service).map_err(|e| Status::from_error(Box::new(e)))?;
        let raw_credentials = credentials
            .map(|c| serde_json::to_vec(&c))
            .transpose()
            .map_err(|e| Status::from_error(Box::new(e)))?;

        let response = GetServiceResponse {
            raw_service,
            raw_credentials,
        };

        Ok(Response::new(response))
    }

    async fn save_service(
        &self,
        req: Request<SaveServiceRequest>,
    ) -> Result<Response<SaveServiceResponse>, Status> {
        let req = req.into_inner();

        if let Some(service) = req.raw_service {
            let service: VersionedServiceTree =
                serde_json::from_slice(&service).map_err(|e| Status::from_error(Box::new(e)))?;
            self.catalog_writer
                .save_service(&req.name, &service)
                .map_err(catalog_error_to_status)?;
        }

        if let Some(credentials) = req.raw_credentials {
            let credentials: Authentication = serde_json::from_slice(&credentials)
                .map_err(|e| Status::from_error(Box::new(e)))?;

            self.catalog_writer
                .save_credentials(&req.name, &credentials)
                .map_err(catalog_error_to_status)?;
        }

        Ok(Response::new(SaveServiceResponse {}))
    }

    async fn run_service(
        &self,
        req: Request<RunServiceRequest>,
    ) -> Result<Response<RunServiceResponse>, Status> {
        let execution_id = uuid::Uuid::new_v4();

        {
            let result = GetRunResultResponse {
                status: get_run_result_response::Status::Running.into(),
                output: None,
            };

            let mut responses = self
                .responses
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            responses.insert(execution_id.to_string(), result);
        };

        let response = RunServiceResponse {
            execution_id: execution_id.to_string(),
        };

        let req = req.into_inner();
        let input =
            serde_json::from_str(&req.input).map_err(|e| Status::from_error(Box::new(e)))?;

        let options = req.limit.map_or(serde_json::Value::Null, |limit| {
            let mut map = serde_json::Map::new();
            map.insert("limit".into(), limit.into());
            map.into()
        });

        let input = from_json(input);
        let options = from_json(options);

        let engine = Arc::clone(&self.engine);
        let responses = Arc::clone(&self.responses);
        let operation_id = req.id;

        // This decides which of the two dispatch paths below to take; it
        // does not do the actual dispatch. `EngineService::is_workflow_operation`
        // does its own locking internally and returns without holding
        // anything, so there's nothing to drop before the `.await` below -
        // see `Engine::is_workflow_operation`'s docs.
        let is_workflow = {
            let ctx = EngineInputContext::new(None, execution_id.to_string(), false);
            engine.is_workflow_operation(&operation_id, &ctx)
        };

        if is_workflow {
            // Genuinely async dispatch: awaited directly on the runtime,
            // never through `spawn_blocking`, since the whole point of a
            // `WorkflowRunner` is not blocking a thread while its steps run
            // concurrently. `Engine::resolve_workflow` (sync) returns owned
            // data with its lock dropped before this task ever awaits
            // anything - see its docs for why holding the lock across the
            // await isn't an option here.
            tokio::spawn(async move {
                let ctx = EngineInputContext::new(None, execution_id.to_string(), false);

                let resolution = engine.resolve_workflow(&operation_id, &ctx);

                finish_run_async(&execution_id.to_string(), &responses, async move {
                    let (service_name, operation_name, workflow, workflow_runner) = resolution?;
                    let result = workflow_runner
                        .run(&service_name, &operation_name, &workflow, input, &ctx)
                        .await?;

                    Ok(wrap_result(to_json(result), ctx.raw_response))
                })
                .await;
            });
        } else {
            tokio::task::spawn_blocking(move || {
                let ctx = EngineInputContext::new(None, execution_id.to_string(), false);

                finish_run(&execution_id.to_string(), &responses, move || {
                    engine.run(&operation_id, input, options, &ctx).map(to_json)
                });
            });
        }

        Ok(Response::new(response))
    }

    async fn get_run_result(
        &self,
        req: Request<GetRunResultRequest>,
    ) -> Result<Response<GetRunResultResponse>, Status> {
        let req = req.into_inner();

        let responses = self
            .responses
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let result = responses
            .get(&req.execution_id)
            .cloned()
            .unwrap_or_else(|| GetRunResultResponse {
                status: get_run_result_response::Status::NotFound.into(),
                output: None,
            });

        Ok(Response::new(result))
    }
}

/// Converts a [`CatalogError`] into the [`Status`] a gRPC handler returns,
/// mirroring how every other error in this crate's handlers is converted.
fn catalog_error_to_status(err: CatalogError) -> Status {
    match err {
        CatalogError::NotFound(msg) => Status::not_found(msg),
        err => Status::from_error(Box::new(err)),
    }
}

/// The tail behavior of an `Engine::run`/`Engine::run_workflow` result: when
/// `raw_response` isn't set, a non-array result is wrapped in a
/// single-element array. This mirrors `execution_engine::wrap_result`
/// exactly, inlined here (rather than depending on the whole
/// `execution_engine` crate for one pure 4-line helper) so this crate's
/// dependency graph stays limited to ports and generated proto types.
fn wrap_result(result: serde_json::Value, raw_response: bool) -> serde_json::Value {
    if raw_response {
        result
    } else if let serde_json::Value::Array(_) = result {
        result
    } else {
        serde_json::Value::Array(vec![result])
    }
}

/// Runs `task`, converts its result into a [`GetRunResultResponse`], and
/// records it under `execution_id` in `responses` — even when `task`
/// panics, so a failing run never leaves its status stuck at `Running`
/// forever.
fn finish_run<F>(
    execution_id: &str,
    responses: &Mutex<HashMap<String, GetRunResultResponse>>,
    task: F,
) where
    F: FnOnce() -> engine::error::Result<serde_json::Value>,
{
    // AssertUnwindSafe is sound here: this function only ever reads `task`'s
    // return value (Ok/Err/panic payload) below, never any state `task`
    // might have partially mutated - there's nothing to observe as
    // inconsistent even if `task` panics mid-way. `Arc<dyn EngineService>`
    // (an opaque trait object) doesn't implement `RefUnwindSafe` on its own,
    // which is what actually requires this.
    let outcome = panic::catch_unwind(panic::AssertUnwindSafe(task));

    let output = match outcome {
        Ok(Ok(result)) => serde_json::to_string_pretty(&result)
            .unwrap_or_else(|err| serde_json::json!({ "error": err.to_string() }).to_string()),
        Ok(Err(err)) => serde_json::json!({ "error": err.to_string() }).to_string(),
        Err(panic) => {
            let msg = panic_message(panic.as_ref());
            serde_json::json!({ "error": format!("panic: {msg}") }).to_string()
        }
    };

    let result = GetRunResultResponse {
        status: get_run_result_response::Status::Completed.into(),
        output: Some(output),
    };

    let mut responses = responses.lock().unwrap_or_else(PoisonError::into_inner);
    responses.insert(execution_id.to_owned(), result);
}

/// The async sibling of [`finish_run`]: awaits `task`, converts its result
/// into a [`GetRunResultResponse`], and records it under `execution_id` in
/// `responses` - even when `task` panics, so a failing workflow run never
/// leaves its status stuck at `Running` forever.
///
/// `task` runs inside its own `tokio::spawn`ed task (rather than being
/// awaited directly) so a panic inside it surfaces as a `JoinError` here
/// instead of unwinding through this task - the async equivalent of
/// [`finish_run`]'s `panic::catch_unwind`.
async fn finish_run_async<F>(
    execution_id: &str,
    responses: &Mutex<HashMap<String, GetRunResultResponse>>,
    task: F,
) where
    F: std::future::Future<Output = engine::error::Result<serde_json::Value>> + Send + 'static,
{
    let outcome = tokio::spawn(task).await;

    let output = match outcome {
        Ok(Ok(result)) => serde_json::to_string_pretty(&result)
            .unwrap_or_else(|err| serde_json::json!({ "error": err.to_string() }).to_string()),
        Ok(Err(err)) => serde_json::json!({ "error": err.to_string() }).to_string(),
        Err(join_err) => {
            let msg = if join_err.is_panic() {
                panic_message(join_err.into_panic().as_ref())
            } else {
                "task cancelled".to_owned()
            };
            serde_json::json!({ "error": format!("panic: {msg}") }).to_string()
        }
    };

    let result = GetRunResultResponse {
        status: get_run_result_response::Status::Completed.into(),
        output: Some(output),
    };

    let mut responses = responses.lock().unwrap_or_else(PoisonError::into_inner);
    responses.insert(execution_id.to_owned(), result);
}

/// Extracts a human-readable message from a caught panic payload, falling
/// back to a generic message for a panic value that isn't a `&str`/`String`
/// (e.g. one raised via `std::panic::panic_any`).
fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_owned()
    }
}

/// Bootstraps and runs the gRPC server on `addr`. This is the entire
/// surface a composition root needs to call to start serving: build the
/// concrete adapters behind [`ServiceCatalog`], [`ServiceCatalogWriter`],
/// and [`EngineService`], then hand them here.
///
/// # Errors
pub async fn serve(
    catalog: Arc<dyn ServiceCatalog + Send + Sync>,
    catalog_writer: Arc<dyn ServiceCatalogWriter + Send + Sync>,
    engine: Arc<dyn EngineService>,
    addr: SocketAddr,
) -> Result<(), tonic::transport::Error> {
    let service = EngineGrpcService::new(catalog, catalog_writer, engine);

    Server::builder()
        .add_service(EngineServer::new(service))
        .serve(addr)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_state() -> (Mutex<HashMap<String, GetRunResultResponse>>, String) {
        let responses = Mutex::new(HashMap::new());

        (responses, "exec-1".to_owned())
    }

    #[test]
    fn finish_run_records_a_completed_response_on_success() {
        let (responses, execution_id) = empty_state();

        finish_run(&execution_id, &responses, || {
            Ok(serde_json::json!({ "hello": "world" }))
        });

        let responses = responses.lock().unwrap();
        let result = responses.get(&execution_id).expect("response recorded");
        assert_eq!(result.status(), get_run_result_response::Status::Completed);
        assert!(result.output.as_ref().unwrap().contains("hello"));
    }

    #[test]
    fn finish_run_records_an_error_response_when_the_task_returns_err() {
        let (responses, execution_id) = empty_state();

        finish_run(&execution_id, &responses, || {
            Err(engine::error::ExecutionEngine::NotFound("widget".into()))
        });

        let responses = responses.lock().unwrap();
        let result = responses.get(&execution_id).expect("response recorded");
        assert_eq!(result.status(), get_run_result_response::Status::Completed);
        assert!(result.output.as_ref().unwrap().contains("widget"));
    }

    #[test]
    fn finish_run_does_not_leave_the_response_stuck_at_running_when_the_task_panics() {
        let (responses, execution_id) = empty_state();

        finish_run(
            &execution_id,
            &responses,
            || -> engine::error::Result<serde_json::Value> {
                panic!("boom");
            },
        );

        let responses = responses.lock().unwrap();
        let result = responses
            .get(&execution_id)
            .expect("a panicking run must still record a response, not leave it stuck at Running");
        assert_eq!(
            result.status(),
            get_run_result_response::Status::Completed,
            "a panicking run must be reported as Completed (with an error), not left Running forever"
        );
        assert!(result.output.as_ref().unwrap().contains("boom"));
    }

    #[tokio::test]
    async fn finish_run_async_records_a_completed_response_on_success() {
        let (responses, execution_id) = empty_state();

        finish_run_async(&execution_id, &responses, async {
            Ok(serde_json::json!({ "hello": "world" }))
        })
        .await;

        let responses = responses.lock().unwrap();
        let result = responses.get(&execution_id).expect("response recorded");
        assert_eq!(result.status(), get_run_result_response::Status::Completed);
        assert!(result.output.as_ref().unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn finish_run_async_records_an_error_response_when_the_task_returns_err() {
        let (responses, execution_id) = empty_state();

        finish_run_async(&execution_id, &responses, async {
            Err(engine::error::ExecutionEngine::NotFound("widget".into()))
        })
        .await;

        let responses = responses.lock().unwrap();
        let result = responses.get(&execution_id).expect("response recorded");
        assert_eq!(result.status(), get_run_result_response::Status::Completed);
        assert!(result.output.as_ref().unwrap().contains("widget"));
    }

    #[tokio::test]
    async fn finish_run_async_does_not_leave_the_response_stuck_at_running_when_the_task_panics() {
        let (responses, execution_id) = empty_state();

        finish_run_async(&execution_id, &responses, async { panic!("boom") }).await;

        let responses = responses.lock().unwrap();
        let result = responses
            .get(&execution_id)
            .expect("a panicking run must still record a response, not leave it stuck at Running");
        assert_eq!(
            result.status(),
            get_run_result_response::Status::Completed,
            "a panicking run must be reported as Completed (with an error), not left Running forever"
        );
        assert!(result.output.as_ref().unwrap().contains("boom"));
    }

    #[test]
    fn wrap_result_wraps_a_non_array_result_when_raw_response_is_unset() {
        let result = wrap_result(serde_json::json!({ "hello": "world" }), false);
        assert!(result.is_array());
    }

    #[test]
    fn wrap_result_leaves_an_array_result_unwrapped() {
        let result = wrap_result(serde_json::json!([1, 2, 3]), false);
        assert_eq!(result, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn wrap_result_leaves_any_result_unwrapped_when_raw_response_is_set() {
        let result = wrap_result(serde_json::json!({ "hello": "world" }), true);
        assert_eq!(result, serde_json::json!({ "hello": "world" }));
    }
}
