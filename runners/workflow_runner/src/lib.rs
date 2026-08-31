//! Adapts `prototypes/workflow_engine::WorkflowEngine` to
//! `execution_engine`'s async `WorkflowRunner` output port - the concrete
//! wiring that connects the standalone prototype crate to the daemon's
//! real dispatch path (`Engine::run_workflow`). A [`CodeRunner`](core_entities::ports::engine::CodeRunner)-style
//! adapter crate like every other `runners/*` crate, just for the
//! `Workflow` manifest kind instead.
//!
//! `mlua::Lua` is `!Sync` unconditionally (its `hook_callback` field is
//! `Option<Arc<dyn Fn(&Lua, Debug) -> mlua::Result<()> + Send>>` - no
//! `+ Sync` - see `mlua-0.9.9/src/types.rs`), so `&Lua` is `!Send`, which
//! makes `WorkflowEngine::run`'s future `!Send` (it holds `&self.lua`
//! across its internal awaits). `WorkflowRunner`'s `#[async_trait]`
//! desugars to `Pin<Box<dyn Future<Output = _> + Send>>` by default, so a
//! `WorkflowAdapter` can't just call `WorkflowEngine::run` inline and
//! `.await` it - that future genuinely isn't `Send`, not a lint false
//! positive (confirmed by reading `mlua`'s source directly, not guessed).
//!
//! The fix is the standard one for embedding `!Send` async work (a
//! thread-affine interpreter, same shape as a GUI toolkit's event loop)
//! inside a `Send`-required multi-threaded async world: confine all Lua
//! work to a dedicated OS thread running its own single-threaded runtime
//! plus a [`tokio::task::LocalSet`], and bridge to/from it with a channel
//! carrying only genuinely `Send` data (an owned manifest, JSON params,
//! and a oneshot reply). [`WorkflowAdapter::run`] itself never touches
//! `Lua` - it only sends a request and awaits the reply - so its future
//! really is `Send`, satisfying [`WorkflowRunner`]'s bound.
//!
//! Every workflow run also gets two host bindings installed fresh per call,
//! alongside `api.step`/`api.join`:
//!
//! - `api.run(id, params, options)`, mirroring `lua_runner`'s binding of
//!   the same name: it lets a workflow script synchronously invoke an
//!   existing `SimpleCode`/`Action`/`Swagger`/etc. operation, of *any*
//!   manifest kind. Unlike `lua_runner`, which calls `engine.run(...)`
//!   inline (safe there because `lua_runner` itself is only ever reached
//!   via `spawn_blocking`), this binding runs on the workflow-dispatch
//!   thread's async runtime, so it bridges into the blocking `Engine::run`
//!   call via its own `tokio::task::spawn_blocking(...).await` - the same
//!   class of bridge #74 called out as necessary, and (per #74's own note)
//!   an easier one to get right than a pure Lua-to-Lua step: the
//!   `spawn_blocking` closure only touches `Arc<dyn EngineService>`/owned
//!   JSON, no Lua state, so it really is `Send + 'static`.
//! - `api.call(id, params, options)` (#75): the genuinely async sibling,
//!   for `Swagger`-kind manifests only. Dispatches directly through the
//!   registered `AsyncDataConnectionRunner` (e.g. `api_caller::AsyncAPICaller`)
//!   with no `spawn_blocking` bridge at all - a real async HTTP client has
//!   no thread-affine state to protect, so the call just `.await`s
//!   in-place. Errors instead of falling back to `api.run` for any other
//!   manifest kind, so a script author's choice between the two bindings
//!   is explicit rather than silently downgraded.

use std::{sync::Arc, time::Duration};

use core_entities::service::WorkflowService as WorkflowManifest;
use core_entities::ports::engine::{
    self, error::ExecutionEngine, DataConnectorBundle, EngineInputContext, EngineService,
    WorkflowRunner,
};
use mlua::LuaSerdeExt;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// `chrono` format string used to timestamp `api.run` log entries -
/// matches `lua_runner::constants::DATETIME_FORMAT`.
const DATETIME_FORMAT: &str = "%a %b %e %Y %I:%M:%S %p";

/// One dispatch request sent to the dedicated workflow thread.
struct WorkflowRequest {
    /// The service name the workflow was invoked as (used as the `api.run`
    /// binding's `EngineInputContext::parent`, so a nested `this.xxx`
    /// identifier resolves against the workflow's own service).
    service_name: String,
    execution_id: String,
    manifest: WorkflowManifest,
    params: Value,
    responder: oneshot::Sender<engine::error::Result<Value>>,
}

/// Sends workflow-run requests to a dedicated single-threaded `LocalSet`
/// where the actual (thread-affine, `!Send`) `mlua`-backed execution
/// happens - see the module docs for why this indirection exists.
pub struct WorkflowAdapter {
    sender: mpsc::UnboundedSender<WorkflowRequest>,
}

impl WorkflowAdapter {
    /// Spawns the dedicated workflow-dispatch thread and returns an
    /// adapter that sends work to it. `engine` is the same
    /// `Arc<dyn EngineService>` `apid` dispatches every other operation
    /// through - it's what the `api.run`/`api.call` bindings bridge into.
    /// The thread runs for the lifetime of the process (detached, not
    /// joined) - it exits on its own once every [`WorkflowAdapter`]
    /// clone/reference holding its sender is dropped and the channel
    /// closes.
    #[must_use]
    pub fn spawn(
        engine: Arc<dyn EngineService>,
        logger: common_data_structures::log_writer::LogWriter,
    ) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();

        std::thread::Builder::new()
            .name("workflow-dispatch".into())
            .spawn(move || run_dispatch_thread(receiver, engine, logger))
            .expect("failed to spawn the workflow-dispatch thread");

        Self { sender }
    }
}

/// The dedicated thread's body: a single-threaded Tokio runtime driving a
/// [`tokio::task::LocalSet`], so every spawned workflow run can freely
/// hold `!Send` `mlua` state without ever needing to cross a thread
/// boundary.
fn run_dispatch_thread(
    mut receiver: mpsc::UnboundedReceiver<WorkflowRequest>,
    engine: Arc<dyn EngineService>,
    logger: common_data_structures::log_writer::LogWriter,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build the workflow-dispatch runtime");
    let local = tokio::task::LocalSet::new();

    local.block_on(&runtime, async move {
        while let Some(request) = receiver.recv().await {
            let engine = Arc::clone(&engine);
            let logger = logger.clone();
            tokio::task::spawn_local(async move {
                let result = run_one_workflow(&request, engine, logger).await;
                // The caller may have gone away (e.g. its request future
                // was dropped) - nothing to do if so.
                let _ = request.responder.send(result);
            });
        }
    });
}

/// Builds a fresh, sandboxed `WorkflowEngine` per call (matching
/// `lua_runner`'s "fresh VM per call is cheap enough" precedent - see #59)
/// using the manifest's own `timeoutSeconds`/`memoryLimitBytes` budget,
/// installs the `api.run` bridge, and runs `params` through it. Runs
/// entirely on the dedicated `LocalSet` thread - see the module docs.
async fn run_one_workflow(
    request: &WorkflowRequest,
    engine: Arc<dyn EngineService>,
    logger: common_data_structures::log_writer::LogWriter,
) -> engine::error::Result<Value> {
    let manifest = &request.manifest;

    let timeout = if manifest.timeout_seconds == 0 {
        DEFAULT_TIMEOUT
    } else {
        Duration::from_secs(u64::from(manifest.timeout_seconds))
    };
    let memory_limit = usize::try_from(manifest.memory_limit_bytes).ok();

    let workflow_engine = workflow_engine::WorkflowEngine::with_limits(timeout, memory_limit)
        .map_err(|err| ExecutionEngine::Other {
            source: anyhow::Error::from(err),
        })?;

    install_api_run_binding(
        &workflow_engine,
        request.service_name.clone(),
        request.execution_id.clone(),
        Arc::clone(&engine),
        logger,
    )
    .map_err(|err| ExecutionEngine::Other {
        source: anyhow::Error::from(err),
    })?;

    install_api_call_binding(
        &workflow_engine,
        request.service_name.clone(),
        request.execution_id.clone(),
        engine,
    )
    .map_err(|err| ExecutionEngine::Other {
        source: anyhow::Error::from(err),
    })?;

    workflow_engine
        .run(manifest.code_string(), request.params.clone())
        .await
        .map_err(|err| ExecutionEngine::Other {
            source: anyhow::Error::from(err),
        })
}

/// Installs `api.run(id, params, options)` on `workflow_engine`, dispatching
/// through `engine` (as a nested call from `service_name`'s running script
/// within `execution_id`) via `tokio::task::spawn_blocking`, and logging
/// each call to `logger` - the same shape as `lua_runner::install_api_binding`,
/// bridged onto an async call site instead of a sync one.
fn install_api_run_binding(
    workflow_engine: &workflow_engine::WorkflowEngine,
    service_name: String,
    execution_id: String,
    engine: Arc<dyn EngineService>,
    logger: common_data_structures::log_writer::LogWriter,
) -> workflow_engine::error::Result<()> {
    workflow_engine.register_api_function("run", move |lua, args: mlua::MultiValue| {
        let engine = Arc::clone(&engine);
        let logger = logger.clone();
        let service_name = service_name.clone();
        let execution_id = execution_id.clone();

        async move {
            let (id, params, options): (String, mlua::Value, Option<mlua::Value>) =
                mlua::FromLuaMulti::from_lua_multi(args, lua)?;

            let now = chrono::offset::Local::now();
            let now = now.format(DATETIME_FORMAT).to_string();
            logger
                .write_all(format!("{now} ({service_name}) [API] {id}\n").as_bytes())
                .map_err(|err| mlua::Error::ExternalError(Arc::new(err)))?;

            let params: serde_json::Value = lua.from_value(params)?;
            let options: serde_json::Value = options
                .map(|value| lua.from_value(value))
                .transpose()?
                .unwrap_or(serde_json::Value::Null);

            let context =
                EngineInputContext::new(Some(service_name.clone()), execution_id.clone(), false);

            let result =
                tokio::task::spawn_blocking(move || engine.run(&id, params, options, &context))
                    .await
                    .map_err(|join_err| mlua::Error::ExternalError(Arc::new(join_err)))?
                    .map_err(|err| mlua::Error::ExternalError(Arc::new(err)))?;

            lua.to_value(&result)
        }
    })
}

/// Installs `api.call(id, params, options)` on `workflow_engine`, dispatching
/// through `engine`'s registered [`AsyncDataConnectionRunner`](core_entities::ports::engine::AsyncDataConnectionRunner)
/// (as a nested call from `service_name`'s running script within
/// `execution_id`) directly on the workflow-dispatch thread's own async
/// runtime - no `spawn_blocking` bridge needed here, unlike `api.run`,
/// since a real async HTTP client has no thread-affine state to protect.
/// Only resolves `Swagger`-kind manifests; anything else (or no async
/// connector registered) errors instead of silently falling back to
/// `api.run`'s behavior, so a script author's choice between the two
/// bindings is explicit.
fn install_api_call_binding(
    workflow_engine: &workflow_engine::WorkflowEngine,
    service_name: String,
    execution_id: String,
    engine: Arc<dyn EngineService>,
) -> workflow_engine::error::Result<()> {
    workflow_engine.register_api_function("call", move |lua, args: mlua::MultiValue| {
        let engine = Arc::clone(&engine);
        let service_name = service_name.clone();
        let execution_id = execution_id.clone();

        async move {
            let (id, params, options): (String, mlua::Value, Option<mlua::Value>) =
                mlua::FromLuaMulti::from_lua_multi(args, lua)?;

            let params: serde_json::Value = lua.from_value(params)?;
            let options: serde_json::Value = options
                .map(|value| lua.from_value(value))
                .transpose()?
                .unwrap_or(serde_json::Value::Null);

            let context =
                EngineInputContext::new(Some(service_name.clone()), execution_id.clone(), false);

            let (resolved_service, operation_name, manifest, api, creds, connector) = engine
                .resolve_data_connector(&id, &context)
                .map_err(|err| mlua::Error::ExternalError(Arc::new(err)))?;

            let bundle = DataConnectorBundle::new(&manifest, &api, creds.as_ref());
            let result = connector
                .run(
                    &resolved_service,
                    &operation_name,
                    &bundle,
                    params,
                    options,
                    &context,
                )
                .await
                .map_err(|err| mlua::Error::ExternalError(Arc::new(err)))?;

            lua.to_value(&result)
        }
    })
}

#[async_trait::async_trait]
impl WorkflowRunner for WorkflowAdapter {
    async fn run(
        &self,
        name: &str,
        _operation_name: &str,
        manifest: &WorkflowManifest,
        params: Value,
        ctx: &EngineInputContext,
    ) -> engine::error::Result<Value> {
        if matches!(
            &manifest.source,
            Some(core_entities::service::workflow_service::Source::ResourcePath(_))
        ) {
            return Err(ExecutionEngine::Unimplemented(
                "Workflow source loaded from a resourcePath (only codeString is supported)".into(),
            ));
        }

        let (tx, rx) = oneshot::channel();
        self.sender
            .send(WorkflowRequest {
                service_name: name.to_owned(),
                execution_id: ctx.execution_id.clone(),
                manifest: manifest.clone(),
                params,
                responder: tx,
            })
            .map_err(|_send_err| ExecutionEngine::Other {
                source: anyhow::anyhow!("the workflow-dispatch thread is not running"),
            })?;

        rx.await.map_err(|_recv_err| ExecutionEngine::Other {
            source: anyhow::anyhow!("the workflow-dispatch thread dropped the response channel"),
        })?
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use core_entities::ports::engine::{AsyncDataConnectionRunner, CodeRunner, EngineLookup};

    use super::*;

    /// Builds a [`WorkflowManifest`] with `code` as its inline Lua source.
    fn workflow_manifest(code: &str) -> WorkflowManifest {
        WorkflowManifest {
            source: Some(core_entities::service::workflow_service::Source::CodeString(
                code.to_owned(),
            )),
            ..Default::default()
        }
    }

    struct EmptyLookup;

    impl EngineLookup for EmptyLookup {
        fn get_service(&self, _id: &str) -> Option<core_entities::service::VersionedServiceTree> {
            None
        }

        fn get_credentials(
            &self,
            _id: &str,
        ) -> Option<credential_entities::credentials::Authentication> {
            None
        }
    }

    fn empty_engine() -> Arc<dyn EngineService> {
        let (logger, _handle) =
            common_data_structures::log_writer::LogWriter::spawn(tempfile::tempfile().unwrap());
        let lookup: Arc<dyn EngineLookup + Send + Sync> = Arc::new(EmptyLookup);
        Arc::new(execution_engine::Engine::new(lookup, logger))
    }

    #[tokio::test]
    async fn workflow_adapter_runs_lua_source_from_the_manifest() {
        let manifest = WorkflowManifest {
            source: Some(core_entities::service::workflow_service::Source::CodeString(
                "return input.x + 1".to_owned(),
            )),
            ..Default::default()
        };

        let (logger, _handle) =
            common_data_structures::log_writer::LogWriter::spawn(tempfile::tempfile().unwrap());
        let adapter = WorkflowAdapter::spawn(empty_engine(), logger);
        let ctx = EngineInputContext::new(None, "exec-1".into(), false);

        let result = adapter
            .run(
                "svc",
                "execute",
                &manifest,
                serde_json::json!({ "x": 41 }),
                &ctx,
            )
            .await
            .expect("workflow run should succeed");

        assert_eq!(result, serde_json::json!(42));
    }

    #[tokio::test]
    async fn workflow_adapter_rejects_a_resource_path_manifest() {
        let manifest = WorkflowManifest {
            source: Some(core_entities::service::workflow_service::Source::ResourcePath(
                "workflow.lua".to_owned(),
            )),
            ..Default::default()
        };

        let (logger, _handle) =
            common_data_structures::log_writer::LogWriter::spawn(tempfile::tempfile().unwrap());
        let adapter = WorkflowAdapter::spawn(empty_engine(), logger);
        let ctx = EngineInputContext::new(None, "exec-1".into(), false);

        let result = adapter
            .run("svc", "execute", &manifest, serde_json::Value::Null, &ctx)
            .await;

        assert!(
            matches!(result, Err(ExecutionEngine::Unimplemented(_))),
            "expected Unimplemented, got {result:?}"
        );
    }

    struct FakeCodeRunner {
        calls: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl CodeRunner for FakeCodeRunner {
        fn run(
            &self,
            name: &str,
            operation_name: &str,
            _source_code: &str,
            params: serde_json::Value,
            _ctx: &EngineInputContext,
        ) -> engine::error::Result<serde_json::Value> {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_owned(), operation_name.to_owned()));
            Ok(params)
        }
    }

    struct SingleServiceLookup(core_entities::service::VersionedServiceTree);

    impl EngineLookup for SingleServiceLookup {
        fn get_service(&self, id: &str) -> Option<core_entities::service::VersionedServiceTree> {
            (id == "other").then(|| self.0.clone())
        }

        fn get_credentials(
            &self,
            _id: &str,
        ) -> Option<credential_entities::credentials::Authentication> {
            None
        }
    }

    /// A `VersionedServiceTree` wrapping a single `SimpleCode` (JavaScript -
    /// any dispatched-to language works equally well here, since the test
    /// registers a `FakeCodeRunner` rather than a real runtime) manifest -
    /// what `api.run` inside a workflow script needs to find via
    /// `Engine::run` for a nested call to actually dispatch anywhere.
    fn simple_code_service() -> core_entities::service::VersionedServiceTree {
        use core_entities::service::{
            code_resource, service_manifest, service_manifest_latest, versioned_service_tree,
            CodeResource, ServiceManifest, ServiceManifestLatest, SimpleCodeService,
            VersionedServiceTree,
        };

        let code = CodeResource {
            language: code_resource::Language::Javascript,
            value: Some(code_resource::Value::CodeString(
                "ignored - FakeCodeRunner doesn't execute it".to_owned(),
            )),
        };

        let simple_code = SimpleCodeService {
            code: Some(code),
            ..Default::default()
        };

        VersionedServiceTree {
            version: Some(versioned_service_tree::Version::V1(
                versioned_service_tree::V1 {
                    manifest: Some(ServiceManifest {
                        value: Some(service_manifest::Value::V2(ServiceManifestLatest {
                            value: Some(service_manifest_latest::Value::SimpleCode(simple_code)),
                            ..Default::default()
                        })),
                    }),
                    ..Default::default()
                },
            )),
        }
    }

    /// A `VersionedServiceTree` wrapping a single `Swagger` manifest with
    /// an empty `CommonApi` - what `api.call` inside a workflow script
    /// needs to find via `Engine::resolve_data_connector` for a nested
    /// call to actually resolve.
    fn swagger_service() -> core_entities::service::VersionedServiceTree {
        use core_entities::service::{
            service_manifest, service_manifest_latest, versioned_service_tree, CommonApi,
            ServiceManifest, ServiceManifestLatest, VersionedServiceTree,
        };

        VersionedServiceTree {
            version: Some(versioned_service_tree::Version::V1(
                versioned_service_tree::V1 {
                    manifest: Some(ServiceManifest {
                        value: Some(service_manifest::Value::V2(ServiceManifestLatest {
                            value: Some(service_manifest_latest::Value::Swagger(Box::default())),
                            ..Default::default()
                        })),
                    }),
                    common_api: Some(CommonApi::default()),
                    ..Default::default()
                },
            )),
        }
    }

    struct FakeAsyncDataConnectionRunner {
        calls: Arc<Mutex<Vec<(String, String)>>>,
    }

    #[async_trait::async_trait]
    impl AsyncDataConnectionRunner for FakeAsyncDataConnectionRunner {
        async fn run(
            &self,
            name: &str,
            operation_name: &str,
            _bundle: &DataConnectorBundle,
            params: serde_json::Value,
            _options: serde_json::Value,
            _ctx: &EngineInputContext,
        ) -> engine::error::Result<serde_json::Value> {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_owned(), operation_name.to_owned()));
            Ok(params)
        }
    }

    #[tokio::test]
    async fn workflow_adapter_api_call_bridges_into_the_registered_async_connector() {
        let (logger, _handle) =
            common_data_structures::log_writer::LogWriter::spawn(tempfile::tempfile().unwrap());
        let lookup: Arc<dyn EngineLookup + Send + Sync> =
            Arc::new(SingleServiceLookup(swagger_service()));
        let mut engine = execution_engine::Engine::new(lookup, logger.clone());

        let calls = Arc::new(Mutex::new(Vec::new()));
        engine.register_async_connector(Arc::new(FakeAsyncDataConnectionRunner {
            calls: Arc::clone(&calls),
        }));
        let engine: Arc<dyn EngineService> = Arc::new(engine);

        let manifest = workflow_manifest("return api.call('other.op', { hello = 'world' })");

        let adapter = WorkflowAdapter::spawn(Arc::clone(&engine), logger);
        let ctx = EngineInputContext::new(None, "exec-1".into(), false);

        let result = adapter
            .run("svc", "execute", &manifest, serde_json::Value::Null, &ctx)
            .await
            .expect("workflow run should succeed");

        assert_eq!(result, serde_json::json!({ "hello": "world" }));
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [("other".to_owned(), "op".to_owned())],
            "expected api.call to reach the fake async connector via the real Engine"
        );
    }

    #[tokio::test]
    async fn workflow_adapter_api_call_errors_for_a_non_swagger_manifest() {
        let (logger, _handle) =
            common_data_structures::log_writer::LogWriter::spawn(tempfile::tempfile().unwrap());
        let lookup: Arc<dyn EngineLookup + Send + Sync> =
            Arc::new(SingleServiceLookup(simple_code_service()));
        let mut engine = execution_engine::Engine::new(lookup, logger.clone());

        let calls = Arc::new(Mutex::new(Vec::new()));
        engine.register_async_connector(Arc::new(FakeAsyncDataConnectionRunner {
            calls: Arc::clone(&calls),
        }));
        let engine: Arc<dyn EngineService> = Arc::new(engine);

        let manifest = workflow_manifest(
            "local ok = pcall(function() return api.call('other.op', {}) end)\n\
                              return { ok = ok }",
        );

        let adapter = WorkflowAdapter::spawn(Arc::clone(&engine), logger);
        let ctx = EngineInputContext::new(None, "exec-1".into(), false);

        let result = adapter
            .run("svc", "execute", &manifest, serde_json::Value::Null, &ctx)
            .await
            .expect("workflow run should succeed - the error is caught by pcall");

        assert_eq!(result, serde_json::json!({ "ok": false }));
        assert!(
            calls.lock().unwrap().is_empty(),
            "the fake connector should never have been called for a non-Swagger manifest"
        );
    }

    #[tokio::test]
    async fn workflow_adapter_api_run_bridges_into_the_existing_sync_engine() {
        let (logger, _handle) =
            common_data_structures::log_writer::LogWriter::spawn(tempfile::tempfile().unwrap());
        let lookup: Arc<dyn EngineLookup + Send + Sync> =
            Arc::new(SingleServiceLookup(simple_code_service()));
        let mut engine = execution_engine::Engine::new(lookup, logger.clone());

        let calls = Arc::new(Mutex::new(Vec::new()));
        engine.register_language(
            "js",
            Box::new(FakeCodeRunner {
                calls: Arc::clone(&calls),
            }),
        );
        let engine: Arc<dyn EngineService> = Arc::new(engine);

        let manifest = workflow_manifest("return api.run('other.op', { hello = 'world' })");

        let adapter = WorkflowAdapter::spawn(Arc::clone(&engine), logger);
        let ctx = EngineInputContext::new(None, "exec-1".into(), false);

        let result = adapter
            .run("svc", "execute", &manifest, serde_json::Value::Null, &ctx)
            .await
            .expect("workflow run should succeed");

        assert_eq!(
            result,
            serde_json::json!([{ "hello": "world" }]),
            "expected Engine::run's usual single-element array wrapping to apply here too, \
             proving api.run really went through Engine::run rather than a shortcut"
        );
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [("other".to_owned(), "op".to_owned())],
            "expected api.run to reach the fake code runner via the real Engine, with the \
             workflow's own service name ('svc') resolving `this.xxx` - not used here since \
             the call already names 'other' explicitly, but proves the bridge is genuinely \
             wired to Engine::run rather than stubbed"
        );
    }
}
