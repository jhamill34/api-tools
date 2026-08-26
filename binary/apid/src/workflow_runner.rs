//! Adapts `prototypes/workflow_engine::WorkflowEngine` to
//! `execution_engine`'s async `WorkflowRunner` output port - the concrete
//! wiring that connects the standalone prototype crate to the daemon's
//! real dispatch path (`Engine::run_workflow`).
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
//! Every workflow run also gets an `api.run(id, params, options)` binding
//! (installed fresh per call, alongside `api.step`/`api.join`), mirroring
//! `lua_runner`'s binding of the same name: it lets a workflow script
//! synchronously invoke an existing `SimpleCode`/`Action`/`Swagger`/etc.
//! operation. Unlike `lua_runner`, which calls `engine.run(...)` inline
//! (safe there because `lua_runner` itself is only ever reached via
//! `spawn_blocking`), this binding runs on the workflow-dispatch thread's
//! async runtime, so it bridges into the blocking `Engine::run` call via
//! its own `tokio::task::spawn_blocking(...).await` - the same class of
//! bridge #74 called out as necessary, and (per #74's own note) an easier
//! one to get right than a pure Lua-to-Lua step: the `spawn_blocking`
//! closure only touches `Arc<RwLock<Engine>>`/owned JSON, no Lua state, so
//! it really is `Send + 'static`.

use std::{
    sync::{Arc, PoisonError, RwLock},
    time::Duration,
};

use core_entities::service::WorkflowService as WorkflowManifest;
use execution_engine::{
    error::ExecutionEngine,
    services::{EngineInputContext, WorkflowRunner},
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
    responder: oneshot::Sender<execution_engine::error::Result<Value>>,
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
    /// `Arc<RwLock<Engine>>` `apid` dispatches every other operation
    /// through - it's what the `api.run` binding bridges into. The thread
    /// runs for the lifetime of the process (detached, not joined) - it
    /// exits on its own once every [`WorkflowAdapter`] clone/reference
    /// holding its sender is dropped and the channel closes.
    #[must_use]
    pub fn spawn(
        engine: Arc<RwLock<execution_engine::Engine>>,
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
    engine: Arc<RwLock<execution_engine::Engine>>,
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
    engine: Arc<RwLock<execution_engine::Engine>>,
    logger: common_data_structures::log_writer::LogWriter,
) -> execution_engine::error::Result<Value> {
    let manifest = &request.manifest;

    let timeout = if manifest.timeoutSeconds == 0 {
        DEFAULT_TIMEOUT
    } else {
        Duration::from_secs(u64::from(manifest.timeoutSeconds))
    };
    let memory_limit = usize::try_from(manifest.memoryLimitBytes).ok();

    let workflow_engine = workflow_engine::WorkflowEngine::with_limits(timeout, memory_limit)
        .map_err(|err| ExecutionEngine::Other {
            source: anyhow::Error::from(err),
        })?;

    install_api_run_binding(
        &workflow_engine,
        request.service_name.clone(),
        request.execution_id.clone(),
        engine,
        logger,
    )
    .map_err(|err| ExecutionEngine::Other {
        source: anyhow::Error::from(err),
    })?;

    workflow_engine
        .run(manifest.codeString(), request.params.clone())
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
    engine: Arc<RwLock<execution_engine::Engine>>,
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

            let result = tokio::task::spawn_blocking(move || {
                let engine = engine.read().unwrap_or_else(PoisonError::into_inner);
                engine.run(&id, params, options, &context)
            })
            .await
            .map_err(|join_err| mlua::Error::ExternalError(Arc::new(join_err)))?
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
    ) -> execution_engine::error::Result<Value> {
        if manifest.has_resourcePath() {
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
