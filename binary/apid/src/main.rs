//! The daemon binary: a `tonic` gRPC server that wires concrete adapters
//! into an [`execution_engine::Engine`] and exposes it as the [`Engine`]
//! service, the composition root of the whole workspace.

mod config;
mod constants;
mod util;
mod workers;

use config::Configuration;

use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    panic,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError, RwLock},
};

use anyhow::{anyhow, Context};
use core_entities::service::VersionedServiceTree;
use credential_entities::credentials::Authentication;
use dotenv::dotenv;
use engine_entities::engine::{
    engine_server::{Engine, EngineServer},
    get_run_result_response,
    list_response::ListItem,
    GetRunResultRequest, GetRunResultResponse, GetSerivceRequest, GetServiceResponse, ListRequest,
    ListResponse, RunServiceRequest, RunServiceResponse, SaveServiceRequest, SaveServiceResponse,
};
use execution_engine::services::EngineLookup;
use in_memory_storage::{repo::InMemoryRepository, OperationRepos};
use local_file_loader::LocalFileFetcher;
use protobuf::Message;
use service_writer::{ServiceWriter, ServiceWriterPort};
use tonic::{transport::Server, Request, Response, Status};

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// Implements the gRPC [`Engine`] service over the shared repositories,
/// execution engine, and in-flight run/signal state.
struct ApiDaemon {
    /// The loaded services and credentials, shared with the background
    /// watcher/loader threads.
    repos: Arc<Mutex<OperationRepos>>,

    /// Each loaded service's directory on disk, keyed by service name.
    paths: Arc<HashMap<String, PathBuf>>,

    /// The execution engine runs are dispatched to.
    engine: Arc<dyn execution_engine::EngineService>,

    /// Results of in-flight and completed runs, keyed by execution ID.
    responses: Arc<Mutex<HashMap<String, GetRunResultResponse>>>,
}

impl ApiDaemon {
    /// Bundles the given shared state into an [`ApiDaemon`].
    #[must_use]
    #[inline]
    fn new(
        repos: Arc<Mutex<OperationRepos>>,
        paths: Arc<HashMap<String, PathBuf>>,
        engine: Arc<dyn execution_engine::EngineService>,
        responses: Arc<Mutex<HashMap<String, GetRunResultResponse>>>,
    ) -> Self {
        Self {
            repos,
            paths,
            engine,
            responses,
        }
    }
}

/// Adapts a shared, lock-guarded [`execution_engine::Engine`] to
/// [`execution_engine::EngineService`]'s unlocked, `&self` contract -
/// mirrors [`LockedLookup`]. `apid` still hands out the concrete
/// `Arc<RwLock<execution_engine::Engine>>` to every `runners/*` adapter that
/// needs to call back into the engine (they depend on the concrete type
/// directly); this wrapper exists only so `ApiDaemon` itself can depend on
/// the driving-port trait instead.
struct LockedEngine(Arc<RwLock<execution_engine::Engine>>);

impl execution_engine::EngineService for LockedEngine {
    fn run(
        &self,
        identifier: &str,
        params: serde_json::Value,
        options: serde_json::Value,
        context: &execution_engine::services::EngineInputContext,
    ) -> execution_engine::error::Result<serde_json::Value> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .run(identifier, params, options, context)
    }

    fn is_workflow_operation(
        &self,
        identifier: &str,
        context: &execution_engine::services::EngineInputContext,
    ) -> bool {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_workflow_operation(identifier, context)
    }

    fn resolve_workflow(
        &self,
        identifier: &str,
        context: &execution_engine::services::EngineInputContext,
    ) -> execution_engine::error::Result<(
        String,
        String,
        core_entities::service::WorkflowService,
        Arc<dyn execution_engine::services::WorkflowRunner>,
    )> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .resolve_workflow(identifier, context)
    }
}

#[tonic::async_trait]
impl Engine for ApiDaemon {
    async fn list(&self, _: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        let repo = self.repos.lock().unwrap_or_else(PoisonError::into_inner);
        let repo = &repo.services;

        let mut items = vec![];

        // The Input Port for Repository
        for id in repo.list() {
            if let Some(service) = repo.get(&id) {
                let service = service.v1();

                let manifest = service.manifest.v2();
                if manifest.has_swagger() {
                    for op_name in service.commonApi.operations.keys() {
                        items.push(ListItem {
                            name: format!("(swagger) {id}.{op_name}"),
                        });
                    }
                }

                if manifest.has_action() {
                    let manifest = manifest.action();

                    for op in &manifest.operations {
                        items.push(ListItem {
                            name: format!("(action) {id}.{}", op.id),
                        });
                    }
                }

                if manifest.has_apiWrapped() {
                    items.push(ListItem {
                        name: format!("(wrapped) {id}.execute"),
                    });
                }

                if manifest.has_simpleCode() {
                    items.push(ListItem {
                        name: format!("(code) {id}.execute"),
                    });
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

        let (service, credentials) = {
            let repo = self.repos.lock().unwrap_or_else(PoisonError::into_inner);
            let services = &repo.services;
            let service = services
                .get(&req.name)
                .ok_or_else(|| Status::not_found("Service not found"))?;

            let credentials = &repo.credentials;
            let creds = credentials.get(&req.name);

            (service, creds)
        };

        let raw_service = service
            .write_to_bytes()
            .map_err(|e| Status::from_error(Box::new(e)))?;
        let raw_credentials = credentials
            .map(|c| c.write_to_bytes())
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

        let location = self
            .paths
            .get(&req.name)
            .ok_or_else(|| Status::not_found("Service location not found"))?;
        let storage = LocalFileFetcher::from(location.clone());

        let writer: Box<dyn ServiceWriterPort<File>> = Box::new(ServiceWriter::default());

        if let Some(service) = req.raw_service {
            let service = VersionedServiceTree::parse_from_bytes(&service)
                .map_err(|e| Status::from_error(Box::new(e)))?;
            writer
                .store_service(&service, &storage, false)
                .map_err(|e| Status::from_error(Box::new(e)))?;
        }

        if let Some(credentials) = req.raw_credentials {
            let credentials = Authentication::parse_from_bytes(&credentials)
                .map_err(|e| Status::from_error(Box::new(e)))?;

            writer
                .store_credentials(&credentials, &storage)
                .map_err(|e| Status::from_error(Box::new(e)))?;
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

        let engine = Arc::clone(&self.engine);
        let responses = Arc::clone(&self.responses);
        let operation_id = req.id;

        // This decides which of the two dispatch paths below to take; it
        // does not do the actual dispatch. `EngineService::is_workflow_operation`
        // does its own locking internally and returns without holding
        // anything, so there's nothing to drop before the `.await` below -
        // see `Engine::is_workflow_operation`'s docs.
        let is_workflow = {
            let ctx = execution_engine::services::EngineInputContext::new(
                None,
                execution_id.to_string(),
                false,
            );
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
                let ctx = execution_engine::services::EngineInputContext::new(
                    None,
                    execution_id.to_string(),
                    false,
                );

                let resolution = engine.resolve_workflow(&operation_id, &ctx);

                finish_run_async(&execution_id.to_string(), &responses, async move {
                    let (service_name, operation_name, workflow, workflow_runner) = resolution?;
                    let result = workflow_runner
                        .run(&service_name, &operation_name, &workflow, input, &ctx)
                        .await?;

                    Ok(execution_engine::wrap_result(result, ctx.raw_response))
                })
                .await;
            });
        } else {
            tokio::task::spawn_blocking(move || {
                let ctx = execution_engine::services::EngineInputContext::new(
                    None,
                    execution_id.to_string(),
                    false,
                );

                finish_run(&execution_id.to_string(), &responses, move || {
                    engine.run(&operation_id, input, options, &ctx)
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

/// Runs `task`, converts its result into a [`GetRunResultResponse`], and
/// records it under `execution_id` in `responses` — even when `task`
/// panics, so a failing run never leaves its status stuck at `Running`
/// forever.
fn finish_run<F>(
    execution_id: &str,
    responses: &Mutex<HashMap<String, GetRunResultResponse>>,
    task: F,
) where
    F: FnOnce() -> execution_engine::error::Result<serde_json::Value>,
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
    F: std::future::Future<Output = execution_engine::error::Result<serde_json::Value>>
        + Send
        + 'static,
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

/// Adapts a shared, mutably-locked [`OperationRepos`] (also written to by
/// the background loader - see [`workers::start_background_watcher`]) to
/// [`EngineLookup`]'s read-only, unlocked contract, encapsulating the lock
/// so [`execution_engine::Engine`] itself never has to know one exists.
struct LockedLookup(Arc<Mutex<OperationRepos>>);

impl EngineLookup for LockedLookup {
    fn get_service(&self, id: &str) -> Option<VersionedServiceTree> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_service(id)
    }

    fn get_credentials(&self, id: &str) -> Option<Authentication> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_credentials(id)
    }
}

/// Builds an [`execution_engine::Engine`] backed by `lookup`, and registers
/// every adapter enabled by this build's Cargo features (the API-call
/// connector is always registered; Python/JavaScript/Lua code runners and
/// the filtered-runner wrapper are each gated behind their own feature
/// flag — `lua` isn't in this build's `default` set yet).
///
/// Does blocking work (`reqwest::blocking::Client::new()` internally
/// spins up its own tokio runtime, which cannot be constructed or torn
/// down from an already-running async context) — callers on the async
/// main thread must invoke this through `tokio::task::spawn_blocking`.
fn construct_execution_engine(
    lookup: Arc<dyn EngineLookup + Sync + Send>,
    workflow_path: &str,
    api_path: &str,
) -> anyhow::Result<Arc<RwLock<execution_engine::Engine>>> {
    let (workflow_logger, _workflow_logger_handle) =
        common_data_structures::log_writer::LogWriter::spawn(File::create(workflow_path)?);

    let (api_logger, _api_logger_handle) =
        common_data_structures::log_writer::LogWriter::spawn(File::create(api_path)?);

    let engine = Arc::new(RwLock::new(execution_engine::Engine::new(
        lookup,
        workflow_logger.clone(),
    )));

    let connector = Box::new(api_caller::APICaller::new(api_logger.clone()));

    #[cfg(feature = "python")]
    let py_runner =
        python_runner::PyActionRunner::new(workflow_logger.clone(), Arc::clone(&engine));

    #[cfg(feature = "javascript")]
    let js_runner =
        javascript_runner::JsActionRunner::new(Arc::clone(&engine), workflow_logger.clone());

    #[cfg(feature = "workflow")]
    let workflow_adapter =
        workflow_runner::WorkflowAdapter::spawn(Arc::clone(&engine), workflow_logger.clone());

    #[cfg(feature = "workflow")]
    let async_connector = Arc::new(api_caller::AsyncAPICaller::new(api_logger));

    #[cfg(feature = "wrapper")]
    let api_wrapper =
        filtered_runner::APIWrapper::new(workflow_logger.clone(), Arc::clone(&engine));

    {
        let mut engine = engine
            .write()
            .map_err(|e| anyhow!("Unable to setup execution engine...: {e}"))?;
        engine.register_connector(connector);

        #[cfg(feature = "python")]
        engine.register_language(constants::PYTHON_LANG, Box::new(py_runner));

        #[cfg(feature = "javascript")]
        engine.register_language(constants::JAVASCRIPT_LANG, Box::new(js_runner));

        #[cfg(feature = "workflow")]
        engine.register_workflow_runner(Arc::new(workflow_adapter));

        #[cfg(feature = "workflow")]
        engine.register_async_connector(async_connector);

        #[cfg(feature = "wrapper")]
        engine.register_filtered_runner(Box::new(api_wrapper));
    };

    Ok(engine)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_err| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    #[cfg(feature = "dhat-ad-hoc")]
    let _profiler = dhat::Profiler::new_ad_hoc();

    dotenv().ok();

    // Setup Singleton Dependencies
    let repos = OperationRepos::new(
        Box::new(InMemoryRepository::new()),
        Box::new(InMemoryRepository::new()),
    );
    let repos = Arc::new(Mutex::new(repos));

    let config_home = env::var(constants::CONFIG_PATH).with_context(|| {
        format!(
            "Unable to get {} environment variable",
            constants::CONFIG_PATH
        )
    })?;
    let config = fs::read_to_string(&config_home)
        .with_context(|| format!("Unable to read config file at {config_home}"))?;
    let config: Configuration = toml::from_str(&config)?;

    let default_path = PathBuf::from(env::var("HOME")?);
    let default_path = default_path.join("./connectors");

    let path = config
        .connector
        .as_ref()
        .map_or(default_path.clone(), |connector| {
            connector
                .path
                .as_ref()
                .map_or(default_path.clone(), PathBuf::from)
        });

    let paths: anyhow::Result<HashMap<String, PathBuf>> = util::get_paths(&path)?
        .map(|dir| {
            let name = dir
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .ok_or_else(|| anyhow!("Unable to get filename from path"))?;
            Ok((name.to_owned(), dir))
        })
        .collect();
    let paths = paths?;
    let paths = Arc::new(paths);

    // Spawn off our background loader
    let (watcher_handler, loader_handler) =
        workers::start_background_watcher(Arc::clone(&repos), &paths)?;

    // TODO: Shard this to reduce lock contention for concurrent requests
    let response_store = Arc::new(Mutex::new(HashMap::<String, GetRunResultResponse>::new()));

    let engine = {
        let repos = Arc::<Mutex<in_memory_storage::OperationRepos>>::clone(&repos);
        let lookup: Arc<dyn EngineLookup + Sync + Send> = Arc::new(LockedLookup(repos));
        let workflow_path = config.log.workflow_path.clone();
        let api_path = config.log.api_path.clone();

        tokio::task::spawn_blocking(move || {
            construct_execution_engine(lookup, &workflow_path, &api_path)
        })
        .await??
    };

    let engine_service: Arc<dyn execution_engine::EngineService> = Arc::new(LockedEngine(engine));
    let engine = ApiDaemon::new(repos, paths, engine_service, response_store);
    let addr = format!("{}:{}", config.server.host, config.server.port).parse()?;

    tracing::info!(%addr, "starting server");

    Server::builder()
        .add_service(EngineServer::new(engine))
        .serve(addr)
        .await?;

    loader_handler
        .join()
        .map_err(|_e| anyhow!("Panic occurred in loader handler"))?;
    watcher_handler
        .join()
        .map_err(|_e| anyhow!("Panic occured in watcher handler"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_state() -> (Mutex<HashMap<String, GetRunResultResponse>>, String) {
        let responses = Mutex::new(HashMap::new());

        (responses, "exec-1".to_owned())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn construct_execution_engine_does_not_panic_when_called_via_spawn_blocking() {
        let repos = OperationRepos::new(
            Box::new(InMemoryRepository::new()),
            Box::new(InMemoryRepository::new()),
        );
        let repos: Arc<dyn EngineLookup + Sync + Send> = Arc::new(repos);

        let log_dir = tempfile::tempdir().unwrap();
        let workflow_path = log_dir
            .path()
            .join("workflow.log")
            .to_string_lossy()
            .into_owned();
        let api_path = log_dir
            .path()
            .join("api.log")
            .to_string_lossy()
            .into_owned();

        // Mirrors exactly how main() calls this: from the async runtime,
        // but through spawn_blocking rather than directly — calling it
        // directly here would reproduce the panic this test guards
        // against (reqwest::blocking::Client::new() cannot construct its
        // own tokio runtime from within an already-running one).
        let result = tokio::task::spawn_blocking(move || {
            construct_execution_engine(repos, &workflow_path, &api_path)
        })
        .await;

        assert!(
            result.is_ok(),
            "expected the spawn_blocking task itself not to panic, got {:?}",
            result.as_ref().err()
        );
        assert!(
            result.unwrap().is_ok(),
            "expected construct_execution_engine to succeed"
        );
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
            Err(execution_engine::error::ExecutionEngine::NotFound(
                "widget".into(),
            ))
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
            || -> execution_engine::error::Result<serde_json::Value> {
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
            Err(execution_engine::error::ExecutionEngine::NotFound(
                "widget".into(),
            ))
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
}
