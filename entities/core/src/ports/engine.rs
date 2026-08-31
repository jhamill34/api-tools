//! The execution engine's primary port ([`EngineService`]) and output
//! ports ([`EngineLookup`], [`DataConnectionRunner`],
//! [`AsyncDataConnectionRunner`], [`CodeRunner`], [`FilteredRunner`],
//! [`WorkflowRunner`], [`ScriptRunner`]), plus the shared types they're
//! called with - moved out of `execution_engine` so a crate that only
//! implements or calls one of these doesn't have to depend on
//! `execution_engine`'s own dispatch logic.

use std::sync::Arc;

use serde_json::Value;

use crate::entity::{
    APIWrappedService, CommonApi, ScriptedAction, SwaggerService, VersionedServiceTree,
    WorkflowService,
};
use credential_entities::entity::Authentication;

/// Errors produced while resolving and running an operation identifier.
pub mod error {
    use std::io;

    use thiserror::Error;

    /// Failure modes of an [`EngineService`](super::EngineService)'s
    /// methods.
    #[derive(Debug, Error)]
    #[non_exhaustive]
    pub enum ExecutionEngine {
        /// The requested service, operation, or adapter wasn't found or
        /// registered.
        #[error("Not found: {0}")]
        NotFound(String),

        /// The manifest used a feature this engine doesn't (yet) support.
        #[error("Unimplemented: {0}")]
        Unimplemented(String),

        /// An operation identifier wasn't in the expected `service.operation`
        /// shape.
        #[error("Invalid Identifier: {0}")]
        InvalidIdentifier(String),

        /// Writing to the shared log file failed.
        #[error(transparent)]
        Io {
            /// The underlying I/O error.
            #[from]
            source: io::Error,
        },

        /// TODO: Rename to `OutputPort`
        #[error(transparent)]
        Other {
            /// The wrapped error from an output port implementation.
            source: anyhow::Error,
        },
    }

    /// Shorthand for a [`Result`](core::result::Result) using
    /// [`ExecutionEngine`] as its error type.
    pub type Result<T> = core::result::Result<T, ExecutionEngine>;
}

/// Context an `Engine` run carries through to whichever output port it
/// dispatches to.
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

/// An input port an `Engine` reads loaded services and credentials from at
/// execution time.
pub trait EngineLookup {
    /// Looks up a loaded service manifest by ID.
    fn get_service(&self, id: &str) -> Option<VersionedServiceTree>;

    /// Looks up loaded credentials by ID.
    fn get_credentials(&self, id: &str) -> Option<Authentication>;
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

impl<'bundle> DataConnectorBundle<'bundle> {
    /// Creates a [`DataConnectorBundle`] from its parts. Needed because the
    /// struct is `#[non_exhaustive]`, which blocks struct-literal
    /// construction from outside this crate - e.g. an
    /// [`AsyncDataConnectionRunner`] implementation's own tests, which
    /// receive a bundle as a parameter in production but need to build one
    /// directly to exercise the trait.
    #[must_use]
    #[inline]
    pub fn new(
        manifest: &'bundle SwaggerService,
        api: &'bundle CommonApi,
        creds: Option<&'bundle Authentication>,
    ) -> Self {
        Self {
            manifest,
            api,
            creds,
        }
    }
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

/// An output port that executes an OpenAPI-backed (`Swagger`) operation
/// without blocking a thread for the call's duration - the async sibling of
/// [`DataConnectionRunner`], for callers that already have somewhere async
/// to run from (e.g. a `WorkflowRunner`'s Lua host bindings). Not dispatched
/// to by the synchronous `Engine::run` - reached only via
/// `Engine::resolve_data_connector`, the same synchronous-resolve-then-await
/// split [`WorkflowRunner`]'s docs describe for the same `!Send`-across-a-lock
/// reason.
#[async_trait::async_trait]
pub trait AsyncDataConnectionRunner: Send + Sync {
    /// Executes `operation_name` against `bundle` with `params`/`options`.
    ///
    /// # Errors
    async fn run(
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
/// `Engine::run` at all. `Engine::run` and every other `*Runner` trait here
/// does blocking work; calling this from inside that synchronous call chain
/// would mean either blocking an async executor thread for the call's
/// duration (if called from async code) or defeating this engine's entire
/// concurrency model by `block_on`-ing it (if called from `Engine::run`'s
/// sync call chain). It's reached only via `Engine::run_workflow`, a
/// separate async entry point a caller `.await`s directly on the async
/// runtime, never through `spawn_blocking`.
#[async_trait::async_trait]
pub trait WorkflowRunner: Send + Sync {
    /// Executes `manifest`'s Lua source with `params`, applying its own
    /// `timeoutSeconds`/`memoryLimitBytes` budget. `name` is the service
    /// name (not the operation name) - matching every sibling `*Runner`
    /// trait's `(name, operation_name, ...)` convention - so an
    /// implementation that bridges back into `Engine::run` (e.g. an
    /// `api.run` binding) can build an [`EngineInputContext`] whose
    /// `parent` correctly resolves a nested `this.xxx` reference.
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
/// but never dispatched to by `Engine::run` - no manifest variant
/// currently routes to it.
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

/// A primary/driving port: the behavioral surface a driving adapter (e.g.
/// `apid`'s gRPC handlers, or a `runners/*` adapter's own bindings calling
/// back in for a nested operation) calls once an `Engine` has been fully
/// built and every adapter registered. Unlike the output-port traits in
/// this module - which `Engine` itself calls *out* through to a registered
/// adapter - this one is implemented *by* `Engine` and called *into* by
/// whoever is driving it, so a caller can depend on this interface instead
/// of the concrete `Engine` type.
pub trait EngineService: Send + Sync {
    /// See `Engine::run`.
    ///
    /// # Errors
    fn run(
        &self,
        identifier: &str,
        params: Value,
        options: Value,
        context: &EngineInputContext,
    ) -> error::Result<Value>;

    /// See `Engine::is_workflow_operation`.
    fn is_workflow_operation(&self, identifier: &str, context: &EngineInputContext) -> bool;

    /// See `Engine::resolve_workflow`.
    ///
    /// # Errors
    #[allow(
        clippy::type_complexity,
        reason = "mirrors Engine::resolve_workflow's own return shape"
    )]
    fn resolve_workflow(
        &self,
        identifier: &str,
        context: &EngineInputContext,
    ) -> error::Result<(String, String, WorkflowService, Arc<dyn WorkflowRunner>)>;

    /// See `Engine::resolve_data_connector`.
    ///
    /// # Errors
    #[allow(
        clippy::type_complexity,
        reason = "mirrors Engine::resolve_data_connector's own return shape"
    )]
    fn resolve_data_connector(
        &self,
        identifier: &str,
        context: &EngineInputContext,
    ) -> error::Result<(
        String,
        String,
        SwaggerService,
        CommonApi,
        Option<Authentication>,
        Arc<dyn AsyncDataConnectionRunner>,
    )>;
}
