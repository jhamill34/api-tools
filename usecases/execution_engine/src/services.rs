//! Output-port traits an [`Engine`](crate::Engine) dispatches manifest
//! steps to, and the shared types they're called with.

use core_entities::service::{
    APIWrappedService, CommonApi, ScriptedAction, SwaggerService, VersionedServiceTree,
    WorkflowService,
};
use credential_entities::credentials::Authentication;
use serde_json::Value;

use crate::error;

/// Context an [`Engine`](crate::Engine) run carries through to whichever
/// output port it dispatches to.
#[derive(Clone)]
#[non_exhaustive]
pub struct EngineInputContext {
    /// The identifier of the service that triggered this run, if any (used
    /// to resolve a `this` service reference against its parent).
    pub parent: Option<String>,

    /// The ID of the top-level execution this run belongs to.
    pub execution_id: String,

    /// If set, the connector's raw response is returned unchanged instead
    /// of being paginated/aggregated.
    pub raw_response: bool,
}

impl EngineInputContext {
    /// Creates an [`EngineInputContext`] from its parts.
    #[must_use]
    #[inline]
    pub fn new(parent: Option<String>, execution_id: String, raw_response: bool) -> Self {
        Self {
            parent,
            execution_id,
            raw_response,
        }
    }
}

/// An input port an [`Engine`](crate::Engine) reads loaded services and
/// credentials from at execution time.
pub trait EngineLookup {
    /// Looks up a loaded service manifest by ID.
    fn get_service(&self, id: &str) -> Option<VersionedServiceTree>;

    /// Looks up loaded credentials by ID.
    fn get_credentials(&self, id: &str) -> Option<Authentication>;
}

/// An output port that pauses execution and waits for an external caller
/// to supply the answer.
pub trait InputPrompter {
    /// Prompts with `params` and blocks until an answer is available.
    ///
    /// # Errors
    fn run(&self, params: Value, ctx: &EngineInputContext) -> error::Result<Value>;
}

/// Everything a [`DataConnectionRunner`] needs to resolve and execute one
/// API-backed operation.
#[non_exhaustive]
pub struct DataConnectorBundle<'bundle> {
    /// The service's `OpenAPI` (`Swagger`) manifest.
    pub manifest: &'bundle SwaggerService,

    /// The service's parsed common API definition (paths, operations,
    /// schemas).
    pub api: &'bundle CommonApi,

    /// The service's credentials, if the operation requires auth.
    pub creds: Option<&'bundle Authentication>,
}

/// An output port that executes an OpenAPI-backed (`Swagger`) operation.
pub trait DataConnectionRunner {
    /// Executes `operation_name` against `bundle` with `params`/`options`.
    ///
    /// # Errors
    fn run(
        &self,
        name: &str,
        operation_name: &str,
        bundle: &DataConnectorBundle,
        params: Value,
        options: Value,
        ctx: &EngineInputContext,
    ) -> error::Result<Value>;
}

/// An output port that executes a `SimpleCode`/`Action` operation's source
/// code in a language-specific runtime.
pub trait CodeRunner {
    /// Executes `source_code` with `params`.
    ///
    /// # Errors
    fn run(
        &self,
        name: &str,
        operation_name: &str,
        source_code: &str,
        params: Value,
        ctx: &EngineInputContext,
    ) -> error::Result<Value>;
}

/// An output port that executes an `ApiWrapped` operation: invokes another
/// already-registered operation and narrows the result to selected output
/// fields.
pub trait FilteredRunner {
    /// Executes `manifest`'s wrapped call with `params`.
    ///
    /// # Errors
    fn run(
        &self,
        name: &str,
        operation_name: &str,
        manifest: &APIWrappedService,
        params: Value,
        ctx: &EngineInputContext,
    ) -> error::Result<Value>;
}

/// An output port that executes a `Workflow` operation's Lua source in an
/// async-native, coroutine-based engine (see `prototypes/workflow_engine`).
///
/// Unlike every other output port in this module, this one is genuinely
/// async - and deliberately not dispatched to by the synchronous
/// [`Engine::run`](crate::Engine::run) at all. `Engine::run` and every
/// other `*Runner` trait here does blocking work; calling this from inside
/// that synchronous call chain would mean either blocking an async
/// executor thread for the call's duration (if called from async code) or
/// defeating this engine's entire concurrency model by `block_on`-ing it
/// (if called from `Engine::run`'s sync call chain). It's reached only via
/// [`Engine::run_workflow`](crate::Engine::run_workflow), a separate async
/// entry point a caller `.await`s directly on the async runtime, never
/// through `spawn_blocking`.
#[async_trait::async_trait]
pub trait WorkflowRunner: Send + Sync {
    /// Executes `manifest`'s Lua source with `params`, applying its own
    /// `timeoutSeconds`/`memoryLimitBytes` budget. `name` is the service
    /// name (not the operation name) - matching every sibling `*Runner`
    /// trait's `(name, operation_name, ...)` convention - so an
    /// implementation that bridges back into [`Engine::run`](crate::Engine::run)
    /// (e.g. an `api.run` binding) can build an [`EngineInputContext`]
    /// whose `parent` correctly resolves a nested `this.xxx` reference.
    ///
    /// # Errors
    async fn run(
        &self,
        name: &str,
        operation_name: &str,
        manifest: &WorkflowService,
        params: Value,
        ctx: &EngineInputContext,
    ) -> error::Result<Value>;
}

/// An output port that executes a `ScriptedAction` operation. Registered
/// but never dispatched to by [`Engine::run`](crate::Engine::run) — no
/// manifest variant currently routes to it.
pub trait ScriptRunner {
    /// # Errors
    fn run(
        &self,
        name: &str,
        operation_name: &str,
        manifest: &ScriptedAction,
        params: Value,
        ctx: &EngineInputContext,
    ) -> error::Result<Value>;
}
