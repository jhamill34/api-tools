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

use std::time::Duration;

use core_entities::service::WorkflowService as WorkflowManifest;
use execution_engine::{
    error::ExecutionEngine,
    services::{EngineInputContext, WorkflowRunner},
};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// One dispatch request sent to the dedicated workflow thread: the
/// manifest and params to run, and where to send the result back.
type WorkflowRequest = (
    WorkflowManifest,
    Value,
    oneshot::Sender<execution_engine::error::Result<Value>>,
);

/// Sends workflow-run requests to a dedicated single-threaded `LocalSet`
/// where the actual (thread-affine, `!Send`) `mlua`-backed execution
/// happens - see the module docs for why this indirection exists.
pub struct WorkflowAdapter {
    sender: mpsc::UnboundedSender<WorkflowRequest>,
}

impl WorkflowAdapter {
    /// Spawns the dedicated workflow-dispatch thread and returns an
    /// adapter that sends work to it. The thread runs for the lifetime of
    /// the process (detached, not joined) - it exits on its own once every
    /// [`WorkflowAdapter`] clone/reference holding its sender is dropped
    /// and the channel closes.
    #[must_use]
    pub fn spawn() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();

        std::thread::Builder::new()
            .name("workflow-dispatch".into())
            .spawn(move || run_dispatch_thread(receiver))
            .expect("failed to spawn the workflow-dispatch thread");

        Self { sender }
    }
}

/// The dedicated thread's body: a single-threaded Tokio runtime driving a
/// [`tokio::task::LocalSet`], so every spawned workflow run can freely
/// hold `!Send` `mlua` state without ever needing to cross a thread
/// boundary.
fn run_dispatch_thread(mut receiver: mpsc::UnboundedReceiver<WorkflowRequest>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build the workflow-dispatch runtime");
    let local = tokio::task::LocalSet::new();

    local.block_on(&runtime, async move {
        while let Some((manifest, params, responder)) = receiver.recv().await {
            tokio::task::spawn_local(async move {
                let result = run_one_workflow(&manifest, params).await;
                // The caller may have gone away (e.g. its request future
                // was dropped) - nothing to do if so.
                let _ = responder.send(result);
            });
        }
    });
}

/// Builds a fresh, sandboxed `WorkflowEngine` per call (matching
/// `lua_runner`'s "fresh VM per call is cheap enough" precedent - see #59)
/// using the manifest's own `timeoutSeconds`/`memoryLimitBytes` budget,
/// and runs `params` through it. Runs entirely on the dedicated
/// `LocalSet` thread - see the module docs.
async fn run_one_workflow(
    manifest: &WorkflowManifest,
    params: Value,
) -> execution_engine::error::Result<Value> {
    let timeout = if manifest.timeoutSeconds == 0 {
        DEFAULT_TIMEOUT
    } else {
        Duration::from_secs(u64::from(manifest.timeoutSeconds))
    };
    let memory_limit = usize::try_from(manifest.memoryLimitBytes).ok();

    let engine =
        workflow_engine::WorkflowEngine::with_limits(timeout, memory_limit).map_err(|err| {
            ExecutionEngine::Other {
                source: anyhow::Error::from(err),
            }
        })?;

    engine
        .run(manifest.codeString(), params)
        .await
        .map_err(|err| ExecutionEngine::Other {
            source: anyhow::Error::from(err),
        })
}

#[async_trait::async_trait]
impl WorkflowRunner for WorkflowAdapter {
    async fn run(
        &self,
        _name: &str,
        manifest: &WorkflowManifest,
        params: Value,
        _ctx: &EngineInputContext,
    ) -> execution_engine::error::Result<Value> {
        if manifest.has_resourcePath() {
            return Err(ExecutionEngine::Unimplemented(
                "Workflow source loaded from a resourcePath (only codeString is supported)".into(),
            ));
        }

        let (tx, rx) = oneshot::channel();
        self.sender
            .send((manifest.clone(), params, tx))
            .map_err(|_send_err| ExecutionEngine::Other {
                source: anyhow::anyhow!("the workflow-dispatch thread is not running"),
            })?;

        rx.await.map_err(|_recv_err| ExecutionEngine::Other {
            source: anyhow::anyhow!("the workflow-dispatch thread dropped the response channel"),
        })?
    }
}
