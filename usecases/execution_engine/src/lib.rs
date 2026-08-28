//! The core orchestrator: [`Engine`] resolves a `service.operation`
//! identifier against a loaded manifest and dispatches it to the
//! registered [`services`] output port for that manifest's type.

pub mod error;
pub mod services;

/// Shared constants for the engine.
mod constants;

use common_data_structures::log_writer::LogWriter;
use serde_json::Value;
use services::{
    AsyncDataConnectionRunner, CodeRunner, DataConnectionRunner, DataConnectorBundle,
    EngineInputContext, EngineLookup, FilteredRunner, ScriptRunner, WorkflowRunner,
};
use std::{collections::HashMap, sync::Arc};

use chrono::offset::Local;
use core_entities::service::{code_resource::Language, service_manifest_latest};

/// Wraps a non-array `result` in a single-element array unless
/// `raw_response` is set - the shared tail behavior of [`Engine::run`] and
/// [`Engine::run_workflow`], also used directly by callers (e.g. `apid`)
/// that resolve and await a [`services::WorkflowRunner`] themselves via
/// [`Engine::resolve_workflow`] instead of going through `run_workflow`.
#[must_use]
pub fn wrap_result(result: Value, raw_response: bool) -> Value {
    if raw_response {
        result
    } else if let Value::Array(_) = result {
        result
    } else {
        Value::Array(vec![result])
    }
}

/// Resolves an operation identifier against a loaded manifest and
/// dispatches it to whichever registered output port matches the
/// manifest's type.
pub struct Engine {
    /// The input port used to resolve a service/its credentials by ID at
    /// execution time. `EngineLookup`'s methods take `&self` only - no
    /// mutation - so this holds no lock of its own; an implementation
    /// backed by mutable state (e.g. `apid`'s background-loaded service
    /// repository) is responsible for its own internal synchronization.
    lookup: Arc<dyn EngineLookup + Send + Sync>,

    /// Where every dispatched run is logged.
    logger: LogWriter,

    /// Handles `OpenAPI` (`Swagger`) operations, if registered.
    connector: Option<Box<dyn DataConnectionRunner + Send + Sync>>,

    /// Handles `SimpleCode`/`Action` operations, keyed by language.
    code_runners: HashMap<String, Box<dyn CodeRunner + Send + Sync>>,

    /// Handles `ScriptedAction` operations, if registered (currently never
    /// dispatched to — see [`ScriptRunner`]).
    script_runner: Option<Box<dyn ScriptRunner + Send + Sync>>,

    /// Handles `ApiWrapped` operations, if registered.
    filtered_runner: Option<Box<dyn FilteredRunner + Send + Sync>>,

    /// Handles `Workflow` operations, if registered. Dispatched to only by
    /// [`Engine::run_workflow`], never by the synchronous [`Engine::run`] -
    /// see [`WorkflowRunner`]'s docs for why.
    ///
    /// `Arc`, not `Box`: [`Engine::resolve_workflow`] clones this out from
    /// behind whatever lock wraps the `Engine` (e.g. `apid`'s
    /// `Arc<std::sync::RwLock<Engine>>`) so the caller can drop that lock
    /// before `.await`-ing the runner - a `Box` couldn't be cloned out this
    /// way.
    workflow_runner: Option<Arc<dyn WorkflowRunner>>,

    /// Handles `Swagger` operations without blocking a thread, if
    /// registered. Dispatched to only by [`Engine::resolve_data_connector`],
    /// never by the synchronous [`Engine::run`] - see
    /// [`AsyncDataConnectionRunner`]'s docs for why. `Arc`, not `Box`, for
    /// the same reason as `workflow_runner` above.
    async_connector: Option<Arc<dyn AsyncDataConnectionRunner>>,
}

impl Engine {
    /// Creates an [`Engine`] with no adapters registered yet; use the
    /// `register_*` methods to add them.
    #[inline]
    pub fn new(lookup: Arc<dyn EngineLookup + Send + Sync>, logger: LogWriter) -> Self {
        Self {
            lookup,
            logger,
            connector: None,
            code_runners: HashMap::new(),
            script_runner: None,
            filtered_runner: None,
            workflow_runner: None,
            async_connector: None,
        }
    }

    /// Registers a [`CodeRunner`] for `lang`,
    /// overwriting any runner already registered for that language.
    #[inline]
    pub fn register_language(&mut self, lang: &str, runner: Box<dyn CodeRunner + Send + Sync>) {
        self.code_runners.insert(lang.to_owned(), runner);
    }

    /// Registers the [`ScriptRunner`].
    #[inline]
    pub fn register_script_runner(&mut self, runner: Box<dyn ScriptRunner + Send + Sync>) {
        self.script_runner = Some(runner);
    }

    /// Registers the [`FilteredRunner`].
    #[inline]
    pub fn register_filtered_runner(&mut self, runner: Box<dyn FilteredRunner + Send + Sync>) {
        self.filtered_runner = Some(runner);
    }

    /// Registers the [`DataConnectionRunner`].
    #[inline]
    pub fn register_connector(&mut self, runner: Box<dyn DataConnectionRunner + Send + Sync>) {
        self.connector = Some(runner);
    }

    /// Registers the [`WorkflowRunner`].
    #[inline]
    pub fn register_workflow_runner(&mut self, runner: Arc<dyn WorkflowRunner>) {
        self.workflow_runner = Some(runner);
    }

    /// Registers the [`AsyncDataConnectionRunner`].
    #[inline]
    pub fn register_async_connector(&mut self, runner: Arc<dyn AsyncDataConnectionRunner>) {
        self.async_connector = Some(runner);
    }

    /// Splits a `service.operation` identifier into its two parts, resolving
    /// a `this` service name against the running context's parent service
    /// (falling back to the literal `"this"` if there is no parent).
    fn parse_identifier<'identifier>(
        identifier: &'identifier str,
        parent: Option<&'identifier str>,
    ) -> error::Result<(&'identifier str, &'identifier str)> {
        let (service_name, operation_name) = identifier
            .split_once('.')
            .ok_or_else(|| error::ExecutionEngine::InvalidIdentifier(identifier.into()))?;

        let service_name = match parent {
            Some(parent) if service_name == "this" => parent,
            _ => service_name,
        };

        Ok((service_name, operation_name))
    }

    /// Resolves `identifier`, looks up the target service and its
    /// credentials, then dispatches to whichever output port matches the
    /// manifest's type (`Swagger` → connector, `Action`/`SimpleCode` →
    /// code runner, `ApiWrapped` → filtered runner). Wraps a non-array
    /// result in a single-element array unless `context.raw_response` is
    /// set.
    ///
    /// # Errors
    #[inline]
    pub fn run(
        &self,
        identifier: &str,
        params: Value,
        options: Value,
        context: &EngineInputContext,
    ) -> error::Result<Value> {
        let (service_name, operation_name) =
            Self::parse_identifier(identifier, context.parent.as_deref())?;

        let service = self
            .lookup
            .get_service(service_name)
            .ok_or_else(|| error::ExecutionEngine::NotFound(identifier.into()))?;
        let credentials = self.lookup.get_credentials(service_name);

        let service = service.v1();
        let manifest = service.manifest.v2();

        let result = match &manifest.value {
            Some(service_manifest_latest::Value::Swagger(swagger)) => self.dispatch_swagger(
                service_name,
                operation_name,
                service,
                swagger,
                credentials,
                params,
                options,
                context,
            ),
            Some(service_manifest_latest::Value::Action(action)) => self.dispatch_action(
                identifier,
                service_name,
                operation_name,
                service,
                action,
                params,
                context,
            ),
            Some(service_manifest_latest::Value::ApiWrapped(api_wrapped)) => self
                .dispatch_api_wrapped(
                    identifier,
                    service_name,
                    operation_name,
                    api_wrapped,
                    params,
                    context,
                ),
            Some(service_manifest_latest::Value::SimpleCode(simple_code)) => self
                .dispatch_simple_code(
                    identifier,
                    service_name,
                    operation_name,
                    simple_code,
                    params,
                    context,
                ),
            _ => Err(error::ExecutionEngine::Unimplemented("API Runner".into())),
        }?;

        Ok(wrap_result(result, context.raw_response))
    }

    /// Dispatches a `Swagger`-kind manifest to the registered
    /// [`DataConnectionRunner`] - the `Swagger` arm of [`Engine::run`]'s
    /// dispatch.
    #[allow(
        clippy::too_many_arguments,
        reason = "each argument is a distinct dispatch input passed through from Engine::run; \
                  see dispatch_code_runner's doc comment above for the same reasoning (#16)"
    )]
    fn dispatch_swagger(
        &self,
        service_name: &str,
        operation_name: &str,
        service: &core_entities::service::versioned_service_tree::V1,
        swagger: &core_entities::service::SwaggerService,
        credentials: Option<credential_entities::credentials::Authentication>,
        params: Value,
        options: Value,
        context: &EngineInputContext,
    ) -> error::Result<Value> {
        if let Some(connector) = &self.connector {
            let api = &service.commonApi;
            let creds = credentials.as_ref();

            let bundle = DataConnectorBundle {
                manifest: swagger,
                api,
                creds,
            };
            connector.run(
                service_name,
                operation_name,
                &bundle,
                params,
                options,
                context,
            )
        } else {
            Err(error::ExecutionEngine::NotFound(
                "Data connector runner".into(),
            ))
        }
    }

    /// Dispatches an `Action`-kind manifest to the registered [`CodeRunner`]
    /// for the resolved operation's language - the `Action` arm of
    /// [`Engine::run`]'s dispatch.
    #[allow(
        clippy::too_many_arguments,
        reason = "each argument is a distinct dispatch input passed through from Engine::run; \
                  see dispatch_code_runner's doc comment above for the same reasoning (#16)"
    )]
    fn dispatch_action(
        &self,
        identifier: &str,
        service_name: &str,
        operation_name: &str,
        service: &core_entities::service::versioned_service_tree::V1,
        action: &core_entities::service::ActionService,
        params: Value,
        context: &EngineInputContext,
    ) -> error::Result<Value> {
        let operation = action
            .operations
            .iter()
            .find(|item| item.id == *operation_name);
        if let Some(operation) = operation {
            let operation = operation.function();

            let path = format!("{}/{}", action.source, operation.js());

            let source = service
                .resources
                .iter()
                .find(|item| item.relativePath == path)
                .ok_or(error::ExecutionEngine::NotFound(format!(
                    "Source file for {service_name}.{operation_name}"
                )))?;

            if let Some(code_runner) = self.code_runners.get(&operation.lang) {
                self.log(identifier, "ACTION", "STARTED")?;
                let result = code_runner.run(
                    service_name,
                    operation_name,
                    &source.content,
                    params,
                    context,
                )?;
                self.log(identifier, "ACTION", "COMPLETED")?;

                Ok(result)
            } else {
                Err(error::ExecutionEngine::NotFound(format!(
                    "Code Runner for language {} not found",
                    operation.lang
                )))
            }
        } else {
            Err(error::ExecutionEngine::NotFound(format!(
                "Action operation {operation_name}"
            )))
        }
    }

    /// Dispatches an `ApiWrapped`-kind manifest to the registered
    /// [`FilteredRunner`] - the `ApiWrapped` arm of [`Engine::run`]'s
    /// dispatch.
    fn dispatch_api_wrapped(
        &self,
        identifier: &str,
        service_name: &str,
        operation_name: &str,
        api_wrapped: &core_entities::service::APIWrappedService,
        params: Value,
        context: &EngineInputContext,
    ) -> error::Result<Value> {
        if let Some(filtered_runner) = &self.filtered_runner {
            self.log(identifier, "API_WRAPPED", "STARTED")?;
            let result =
                filtered_runner.run(service_name, operation_name, api_wrapped, params, context)?;
            self.log(identifier, "API_WRAPPED", "COMPLETED")?;

            Ok(result)
        } else {
            Err(error::ExecutionEngine::NotFound(
                "API Wrapper runner not found".into(),
            ))
        }
    }

    /// Dispatches a `SimpleCode`-kind manifest to the [`CodeRunner`]
    /// registered for its `language` - the `SimpleCode` arm of
    /// [`Engine::run`]'s dispatch.
    fn dispatch_simple_code(
        &self,
        identifier: &str,
        service_name: &str,
        operation_name: &str,
        simple_code: &core_entities::service::SimpleCodeService,
        params: Value,
        context: &EngineInputContext,
    ) -> error::Result<Value> {
        match simple_code.code.language.enum_value() {
            Ok(Language::PYTHON) => self.dispatch_code_runner(
                identifier,
                service_name,
                operation_name,
                "python",
                simple_code.code.codeString(),
                params,
                context,
            ),
            Ok(Language::JAVASCRIPT) => self.dispatch_code_runner(
                identifier,
                service_name,
                operation_name,
                "js",
                simple_code.code.codeString(),
                params,
                context,
            ),
            // LUA is deliberately not dispatched to here - see #73:
            // `Workflow`-kind manifests (via `WorkflowRunner`) are
            // the replacement for Lua `SimpleCode` operations, not
            // a second parallel Lua execution path through this
            // arm. The `LUA` enum variant itself stays defined
            // (harmless, and a smaller footprint than removing a
            // wire enum value), it's just unreachable here now.
            _ => Err(error::ExecutionEngine::NotFound("Unknown language".into())),
        }
    }

    /// Resolves `identifier` against a `Workflow`-kind manifest, returning
    /// the fully **owned** pieces (`service_name`, `operation_name`, the
    /// manifest's cloned `WorkflowService`, and the registered
    /// `WorkflowRunner` cloned out of its `Arc`) a caller needs to run it -
    /// entirely synchronously, with every lock this method touches (the
    /// `lookup` mutex, and whatever lock wraps the `Engine` itself in the
    /// caller, e.g. `apid`'s `Arc<std::sync::RwLock<Engine>>`) dropped
    /// before it returns.
    ///
    /// This split exists so a caller can `.await` the returned runner with
    /// *zero* locks held across the await point: holding a
    /// `std::sync::RwLockReadGuard` (which is `!Send`) across an `.await`
    /// makes the containing future non-`Send`, which fails to compile under
    /// `tonic`'s `#[tonic::async_trait]`-generated service methods. See
    /// [`WorkflowRunner`]'s docs for the broader reasoning.
    ///
    /// # Errors
    /// Returns an error if the identifier can't be parsed, the service
    /// isn't found, the manifest isn't a `Workflow`, or no
    /// [`WorkflowRunner`] is registered.
    pub fn resolve_workflow(
        &self,
        identifier: &str,
        context: &EngineInputContext,
    ) -> error::Result<(
        String,
        String,
        core_entities::service::WorkflowService,
        Arc<dyn WorkflowRunner>,
    )> {
        let (service_name, operation_name) =
            Self::parse_identifier(identifier, context.parent.as_deref())?;

        let service = self
            .lookup
            .get_service(service_name)
            .ok_or_else(|| error::ExecutionEngine::NotFound(identifier.into()))?;
        let service = service.v1();
        let manifest = service.manifest.v2();

        match &manifest.value {
            Some(service_manifest_latest::Value::Workflow(workflow)) => {
                let workflow_runner = self.workflow_runner.clone().ok_or_else(|| {
                    error::ExecutionEngine::NotFound("Workflow runner not registered".into())
                })?;

                Ok((
                    service_name.to_owned(),
                    operation_name.to_owned(),
                    workflow.clone(),
                    workflow_runner,
                ))
            }
            _ => Err(error::ExecutionEngine::Unimplemented(
                "resolve_workflow called on a non-Workflow manifest".into(),
            )),
        }
    }

    /// Resolves `identifier` and dispatches to the registered
    /// [`WorkflowRunner`], for `Workflow`-kind manifests only.
    ///
    /// A separate, genuinely async entry point from [`Engine::run`] -
    /// callers `.await` this directly on the async runtime, never through
    /// `spawn_blocking`, since the whole point of a [`WorkflowRunner`] is
    /// not blocking a thread while its steps run concurrently. See
    /// [`WorkflowRunner`]'s docs for the full reasoning.
    ///
    /// Thin wrapper around [`Engine::resolve_workflow`] (sync lookup) +
    /// awaiting the resolved runner - see that method's docs for why the
    /// split matters.
    ///
    /// # Errors
    /// Returns an error if the identifier can't be parsed, the service
    /// isn't found, the manifest isn't a `Workflow`, or no
    /// [`WorkflowRunner`] is registered.
    pub async fn run_workflow(
        &self,
        identifier: &str,
        params: Value,
        context: &EngineInputContext,
    ) -> error::Result<Value> {
        let (service_name, operation_name, workflow, workflow_runner) =
            self.resolve_workflow(identifier, context)?;

        self.log(identifier, "WORKFLOW", "STARTED")?;
        let result = workflow_runner
            .run(&service_name, &operation_name, &workflow, params, context)
            .await?;
        self.log(identifier, "WORKFLOW", "COMPLETED")?;

        Ok(wrap_result(result, context.raw_response))
    }

    /// Reports whether `identifier` resolves to a `Workflow`-kind manifest.
    ///
    /// Used by callers (e.g. `apid::run_service`) to decide, cheaply and
    /// synchronously, whether to dispatch through the async
    /// [`Engine::resolve_workflow`] path or the legacy synchronous
    /// [`Engine::run`] path - before doing any real dispatch work. Returns
    /// `false` for every resolution failure (an unparseable identifier, an
    /// unknown service) rather than an error, so callers fall through to
    /// [`Engine::run`]'s own, already-established error reporting for
    /// those cases instead of this method duplicating it.
    #[must_use]
    pub fn is_workflow_operation(&self, identifier: &str, context: &EngineInputContext) -> bool {
        let Ok((service_name, _operation_name)) =
            Self::parse_identifier(identifier, context.parent.as_deref())
        else {
            return false;
        };

        let Some(service) = self.lookup.get_service(service_name) else {
            return false;
        };

        let service = service.v1();
        let manifest = service.manifest.v2();

        matches!(
            &manifest.value,
            Some(service_manifest_latest::Value::Workflow(_))
        )
    }

    /// Resolves `identifier` against a `Swagger`-kind manifest, returning
    /// the fully **owned** pieces (`service_name`, `operation_name`, the
    /// manifest's cloned `SwaggerService` and `CommonApi`, cloned
    /// credentials, and the registered [`AsyncDataConnectionRunner`] cloned
    /// out of its `Arc`) a caller needs to run it - entirely synchronously,
    /// with every lock this method touches dropped before it returns. Same
    /// split, and the same reason for it, as [`Engine::resolve_workflow`]:
    /// it lets a caller (e.g. a `WorkflowRunner`'s `api.call` binding)
    /// `.await` the returned runner with zero `Engine`-level locks held.
    ///
    /// # Errors
    /// Returns an error if the identifier can't be parsed, the service
    /// isn't found, the manifest isn't a `Swagger` manifest, or no
    /// [`AsyncDataConnectionRunner`] is registered.
    #[allow(
        clippy::type_complexity,
        reason = "return type mirrors the resolved connector's own nested Result/Option shape"
    )]
    pub fn resolve_data_connector(
        &self,
        identifier: &str,
        context: &EngineInputContext,
    ) -> error::Result<(
        String,
        String,
        core_entities::service::SwaggerService,
        core_entities::service::CommonApi,
        Option<credential_entities::credentials::Authentication>,
        Arc<dyn AsyncDataConnectionRunner>,
    )> {
        let (service_name, operation_name) =
            Self::parse_identifier(identifier, context.parent.as_deref())?;

        let service = self
            .lookup
            .get_service(service_name)
            .ok_or_else(|| error::ExecutionEngine::NotFound(identifier.into()))?;
        let credentials = self.lookup.get_credentials(service_name);

        let service = service.v1();
        let manifest = service.manifest.v2();

        match &manifest.value {
            Some(service_manifest_latest::Value::Swagger(swagger)) => {
                let async_connector = self.async_connector.clone().ok_or_else(|| {
                    error::ExecutionEngine::NotFound("Async data connector not registered".into())
                })?;

                Ok((
                    service_name.to_owned(),
                    operation_name.to_owned(),
                    swagger.clone(),
                    (*service.commonApi).clone(),
                    credentials,
                    async_connector,
                ))
            }
            _ => Err(error::ExecutionEngine::Unimplemented(
                "resolve_data_connector called on a non-Swagger manifest".into(),
            )),
        }
    }

    /// Dispatches to the [`CodeRunner`] registered under `lang_key`,
    /// logging `SIMPLE_CODE` start/completion - the shared body of
    /// [`Engine::run`]'s `SimpleCode` dispatch, parameterized by language
    /// so `python`/`javascript` (and any future language) share one
    /// implementation instead of near-identical copies (see #16).
    ///
    /// # Errors
    /// Returns an error if no `CodeRunner` is registered for `lang_key`, or
    /// if the registered runner's own call fails.
    #[allow(
        clippy::too_many_arguments,
        reason = "each argument is a distinct dispatch input; see the doc comment above for why \
                  this isn't yet unified with the sibling dispatch fn (#16)"
    )]
    fn dispatch_code_runner(
        &self,
        identifier: &str,
        service_name: &str,
        operation_name: &str,
        lang_key: &str,
        source_code: &str,
        params: Value,
        context: &EngineInputContext,
    ) -> error::Result<Value> {
        let code_runner = self.code_runners.get(lang_key).ok_or_else(|| {
            error::ExecutionEngine::NotFound(format!("Code runner not found for {lang_key}"))
        })?;

        self.log(identifier, "SIMPLE_CODE", "STARTED")?;
        let result = code_runner.run(service_name, operation_name, source_code, params, context)?;
        self.log(identifier, "SIMPLE_CODE", "COMPLETED")?;

        Ok(result)
    }

    /// Queues a timestamped `(action_type) [status] id` line to the shared
    /// log writer.
    fn log(&self, id: &str, action_type: &str, status: &str) -> error::Result<()> {
        let now = Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        self.logger
            .write_all(format!("{now} ({action_type}) [{status}] {id}\n").as_bytes())?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_identifier_splits_service_and_operation() {
        let result = Engine::parse_identifier("myservice.myoperation", None);

        assert!(matches!(result, Ok(("myservice", "myoperation"))));
    }

    #[test]
    fn parse_identifier_resolves_this_against_the_parent() {
        let result = Engine::parse_identifier("this.myoperation", Some("parent_service"));

        assert!(matches!(result, Ok(("parent_service", "myoperation"))));
    }

    #[test]
    fn parse_identifier_keeps_the_literal_this_when_there_is_no_parent() {
        let result = Engine::parse_identifier("this.myoperation", None);

        assert!(matches!(result, Ok(("this", "myoperation"))));
    }

    #[test]
    fn parse_identifier_rejects_an_identifier_with_no_dot() {
        let result = Engine::parse_identifier("noDotHere", None);

        assert!(
            matches!(result, Err(error::ExecutionEngine::InvalidIdentifier(_))),
            "expected InvalidIdentifier, got {result:?}"
        );
    }

    struct FakeLookup;

    impl EngineLookup for FakeLookup {
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

    #[test]
    fn log_queues_a_line_through_the_shared_log_writer_instead_of_blocking_on_a_lock() {
        let file = tempfile::tempfile().unwrap();
        let (writer, handle) =
            common_data_structures::log_writer::LogWriter::spawn(file.try_clone().unwrap());

        let lookup: Arc<dyn EngineLookup + Send + Sync> = Arc::new(FakeLookup);
        let engine = Engine::new(lookup, writer.clone());

        engine.log("svc.op", "ACTION", "STARTED").unwrap();

        drop(writer);
        drop(engine);
        handle.join().unwrap();

        let mut file = file;
        let mut contents = String::new();
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0)).unwrap();
        std::io::Read::read_to_string(&mut file, &mut contents).unwrap();

        assert!(
            contents.contains("(ACTION) [STARTED] svc.op"),
            "expected the log line in {contents:?}"
        );
    }

    /// Builds a [`VersionedServiceTree`] wrapping a single `Workflow`
    /// manifest with `code` as its Lua source.
    fn workflow_service(code: &str) -> core_entities::service::VersionedServiceTree {
        let mut manifest = core_entities::service::ServiceManifest::new();
        manifest
            .mut_v2()
            .mut_workflow()
            .set_codeString(code.to_owned());

        let mut tree = core_entities::service::VersionedServiceTree::new();
        tree.mut_v1().manifest = protobuf::MessageField::some(manifest);
        tree
    }

    struct WorkflowLookup(core_entities::service::VersionedServiceTree);

    impl EngineLookup for WorkflowLookup {
        fn get_service(&self, _id: &str) -> Option<core_entities::service::VersionedServiceTree> {
            Some(self.0.clone())
        }

        fn get_credentials(
            &self,
            _id: &str,
        ) -> Option<credential_entities::credentials::Authentication> {
            None
        }
    }

    struct FakeWorkflowRunner {
        calls: std::sync::Arc<std::sync::Mutex<Vec<(String, String, String)>>>,
    }

    #[async_trait::async_trait]
    impl WorkflowRunner for FakeWorkflowRunner {
        async fn run(
            &self,
            name: &str,
            operation_name: &str,
            manifest: &core_entities::service::WorkflowService,
            _params: Value,
            _ctx: &EngineInputContext,
        ) -> error::Result<Value> {
            self.calls.lock().unwrap().push((
                name.to_owned(),
                operation_name.to_owned(),
                manifest.codeString().to_owned(),
            ));
            Ok(Value::String("workflow ran".into()))
        }
    }

    fn test_logger() -> LogWriter {
        let file = tempfile::tempfile().unwrap();
        let (writer, _handle) = LogWriter::spawn(file);
        writer
    }

    /// Builds a [`VersionedServiceTree`] wrapping a single `SimpleCode`
    /// manifest in `language`.
    fn simple_code_service(
        language: core_entities::service::code_resource::Language,
    ) -> core_entities::service::VersionedServiceTree {
        let mut code = core_entities::service::CodeResource::new();
        code.set_codeString("return 1".to_owned());
        code.language = protobuf::EnumOrUnknown::new(language);

        let mut simple_code = core_entities::service::SimpleCodeService::new();
        simple_code.code = protobuf::MessageField::some(code);

        let mut manifest = core_entities::service::ServiceManifest::new();
        manifest.mut_v2().set_simpleCode(simple_code);

        let mut tree = core_entities::service::VersionedServiceTree::new();
        tree.mut_v1().manifest = protobuf::MessageField::some(manifest);
        tree
    }

    #[test]
    fn run_reports_the_correct_language_when_no_javascript_code_runner_is_registered() {
        let lookup: Arc<dyn EngineLookup + Send + Sync> = Arc::new(WorkflowLookup(
            simple_code_service(core_entities::service::code_resource::Language::JAVASCRIPT),
        ));
        let engine = Engine::new(lookup, test_logger());

        let ctx = EngineInputContext::new(None, "exec-1".into(), false);
        let result = engine.run("svc.execute", Value::Null, Value::Null, &ctx);

        let err = result.expect_err("expected an error since no js runner is registered");
        assert_eq!(
            err.to_string(),
            "Not found: Code runner not found for js",
            "the JAVASCRIPT branch's error message previously wrongly said 'python' - #16"
        );
    }

    #[tokio::test]
    async fn run_workflow_dispatches_to_the_registered_workflow_runner() {
        let lookup: Arc<dyn EngineLookup + Send + Sync> =
            Arc::new(WorkflowLookup(workflow_service("return 42")));
        let mut engine = Engine::new(lookup, test_logger());

        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        engine.register_workflow_runner(Arc::new(FakeWorkflowRunner {
            calls: std::sync::Arc::clone(&calls),
        }));

        let ctx = EngineInputContext::new(None, "exec-1".into(), false);
        let result = engine
            .run_workflow("svc.execute", Value::Null, &ctx)
            .await
            .expect("run_workflow should succeed");

        assert_eq!(
            result,
            Value::Array(vec![Value::String("workflow ran".into())])
        );
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [(
                "svc".to_owned(),
                "execute".to_owned(),
                "return 42".to_owned()
            )],
            "expected the runner to receive the service name and operation name as two \
             distinct arguments, not the operation name in both"
        );
    }

    #[tokio::test]
    async fn run_workflow_errors_when_no_workflow_runner_is_registered() {
        let lookup: Arc<dyn EngineLookup + Send + Sync> =
            Arc::new(WorkflowLookup(workflow_service("return 42")));
        let engine = Engine::new(lookup, test_logger());

        let ctx = EngineInputContext::new(None, "exec-1".into(), false);
        let result = engine.run_workflow("svc.execute", Value::Null, &ctx).await;

        assert!(
            matches!(result, Err(error::ExecutionEngine::NotFound(_))),
            "expected NotFound, got {result:?}"
        );
    }

    #[tokio::test]
    async fn run_workflow_errors_when_the_manifest_is_not_a_workflow() {
        let mut manifest = core_entities::service::ServiceManifest::new();
        manifest.mut_v2().mut_swagger();
        let mut tree = core_entities::service::VersionedServiceTree::new();
        tree.mut_v1().manifest = protobuf::MessageField::some(manifest);

        let lookup: Arc<dyn EngineLookup + Send + Sync> = Arc::new(WorkflowLookup(tree));
        let mut engine = Engine::new(lookup, test_logger());
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        engine.register_workflow_runner(Arc::new(FakeWorkflowRunner {
            calls: Arc::clone(&calls),
        }));

        let ctx = EngineInputContext::new(None, "exec-1".into(), false);
        let result = engine.run_workflow("svc.execute", Value::Null, &ctx).await;

        assert!(
            matches!(result, Err(error::ExecutionEngine::Unimplemented(_))),
            "expected Unimplemented, got {result:?}"
        );
        assert!(
            calls.lock().unwrap().is_empty(),
            "the workflow runner should never have been called for a non-Workflow manifest"
        );
    }

    #[test]
    fn is_workflow_operation_is_true_for_a_workflow_manifest() {
        let lookup: Arc<dyn EngineLookup + Send + Sync> =
            Arc::new(WorkflowLookup(workflow_service("return 42")));
        let engine = Engine::new(lookup, test_logger());

        let ctx = EngineInputContext::new(None, "exec-1".into(), false);

        assert!(engine.is_workflow_operation("svc.execute", &ctx));
    }

    #[test]
    fn is_workflow_operation_is_false_for_a_non_workflow_manifest() {
        let mut manifest = core_entities::service::ServiceManifest::new();
        manifest.mut_v2().mut_swagger();
        let mut tree = core_entities::service::VersionedServiceTree::new();
        tree.mut_v1().manifest = protobuf::MessageField::some(manifest);

        let lookup: Arc<dyn EngineLookup + Send + Sync> = Arc::new(WorkflowLookup(tree));
        let engine = Engine::new(lookup, test_logger());

        let ctx = EngineInputContext::new(None, "exec-1".into(), false);

        assert!(!engine.is_workflow_operation("svc.execute", &ctx));
    }

    #[test]
    fn is_workflow_operation_is_false_when_the_service_is_not_found() {
        let lookup: Arc<dyn EngineLookup + Send + Sync> = Arc::new(FakeLookup);
        let engine = Engine::new(lookup, test_logger());

        let ctx = EngineInputContext::new(None, "exec-1".into(), false);

        assert!(!engine.is_workflow_operation("svc.execute", &ctx));
    }

    #[test]
    fn is_workflow_operation_is_false_for_an_unparseable_identifier() {
        let lookup: Arc<dyn EngineLookup + Send + Sync> = Arc::new(FakeLookup);
        let engine = Engine::new(lookup, test_logger());

        let ctx = EngineInputContext::new(None, "exec-1".into(), false);

        assert!(!engine.is_workflow_operation("noDotHere", &ctx));
    }

    /// Builds a [`VersionedServiceTree`] wrapping a single `Swagger`
    /// manifest with an empty `CommonApi`.
    fn swagger_service() -> core_entities::service::VersionedServiceTree {
        let mut manifest = core_entities::service::ServiceManifest::new();
        manifest.mut_v2().mut_swagger();

        let mut tree = core_entities::service::VersionedServiceTree::new();
        tree.mut_v1().manifest = protobuf::MessageField::some(manifest);
        tree.mut_v1().commonApi =
            protobuf::MessageField::some(core_entities::service::CommonApi::new());
        tree
    }

    struct FakeAsyncDataConnectionRunner {
        calls: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }

    #[async_trait::async_trait]
    impl services::AsyncDataConnectionRunner for FakeAsyncDataConnectionRunner {
        async fn run(
            &self,
            name: &str,
            operation_name: &str,
            _bundle: &services::DataConnectorBundle,
            _params: Value,
            _options: Value,
            _ctx: &EngineInputContext,
        ) -> error::Result<Value> {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_owned(), operation_name.to_owned()));
            Ok(Value::String("connector called".into()))
        }
    }

    #[test]
    fn resolve_data_connector_returns_owned_pieces_for_a_swagger_manifest() {
        let lookup: Arc<dyn EngineLookup + Send + Sync> =
            Arc::new(WorkflowLookup(swagger_service()));
        let mut engine = Engine::new(lookup, test_logger());

        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        engine.register_async_connector(Arc::new(FakeAsyncDataConnectionRunner {
            calls: Arc::clone(&calls),
        }));

        let ctx = EngineInputContext::new(None, "exec-1".into(), false);
        let (service_name, operation_name, _manifest, _api, _creds, _runner) = engine
            .resolve_data_connector("svc.execute", &ctx)
            .expect("resolve_data_connector should succeed");

        assert_eq!(service_name, "svc");
        assert_eq!(operation_name, "execute");
        assert!(
            calls.lock().unwrap().is_empty(),
            "resolve_data_connector should only resolve, never call the runner itself"
        );
    }

    #[test]
    fn resolve_data_connector_errors_when_no_async_connector_is_registered() {
        let lookup: Arc<dyn EngineLookup + Send + Sync> =
            Arc::new(WorkflowLookup(swagger_service()));
        let engine = Engine::new(lookup, test_logger());

        let ctx = EngineInputContext::new(None, "exec-1".into(), false);
        let result = engine.resolve_data_connector("svc.execute", &ctx);

        assert!(
            matches!(result, Err(error::ExecutionEngine::NotFound(_))),
            "expected NotFound, got {:?}",
            result.err()
        );
    }

    #[test]
    fn resolve_data_connector_errors_when_the_manifest_is_not_swagger() {
        let lookup: Arc<dyn EngineLookup + Send + Sync> =
            Arc::new(WorkflowLookup(workflow_service("return 42")));
        let mut engine = Engine::new(lookup, test_logger());

        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        engine.register_async_connector(Arc::new(FakeAsyncDataConnectionRunner {
            calls: Arc::clone(&calls),
        }));

        let ctx = EngineInputContext::new(None, "exec-1".into(), false);
        let result = engine.resolve_data_connector("svc.execute", &ctx);

        assert!(
            matches!(result, Err(error::ExecutionEngine::Unimplemented(_))),
            "expected Unimplemented, got {:?}",
            result.err()
        );
    }
}
