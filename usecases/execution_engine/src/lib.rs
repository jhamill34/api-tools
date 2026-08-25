#![warn(clippy::restriction, clippy::pedantic)]
#![allow(
    clippy::blanket_clippy_restriction_lints,
    clippy::mod_module_files,
    clippy::self_named_module_files,

    clippy::implicit_return,
    clippy::shadow_reuse,
    clippy::match_ref_pats,

    // Would like to turn on (Configured to 50?)
    clippy::too_many_lines,
    clippy::needless_borrowed_reference,
    clippy::question_mark_used,
    clippy::ref_patterns
)]

//! The core orchestrator: [`Engine`] resolves a `service.operation`
//! identifier against a loaded manifest and dispatches it to the
//! registered [`services`] output port for that manifest's type.

pub mod error;
pub mod services;

/// Shared constants for the engine.
mod constants;

extern crate alloc;
use alloc::sync::Arc;

use common_data_structures::log_writer::LogWriter;
use serde_json::Value;
use services::{
    CodeRunner, DataConnectionRunner, DataConnectorBundle, EngineInputContext, EngineLookup,
    FilteredRunner, InputPrompter, ScriptRunner,
};
use std::{collections::HashMap, sync::Mutex};

use chrono::offset::Local;
use core_entities::service::{code_resource::Language, service_manifest_latest};

/// Resolves an operation identifier against a loaded manifest and
/// dispatches it to whichever registered output port matches the
/// manifest's type.
pub struct Engine {
    /// The input port used to resolve a service/its credentials by ID at
    /// execution time.
    lookup: Arc<Mutex<dyn EngineLookup + Send + Sync>>,

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

    /// Handles the built-in `$input` operation, if registered.
    input_handler: Option<Box<dyn InputPrompter + Send + Sync>>,
}

impl Engine {
    /// Creates an [`Engine`] with no adapters registered yet; use the
    /// `register_*` methods to add them.
    #[inline]
    pub fn new(lookup: Arc<Mutex<dyn EngineLookup + Send + Sync>>, logger: LogWriter) -> Self {
        Self {
            lookup,
            logger,
            connector: None,
            code_runners: HashMap::new(),
            script_runner: None,
            filtered_runner: None,
            input_handler: None,
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

    /// Registers the [`InputPrompter`].
    #[inline]
    pub fn register_input(&mut self, handler: Box<dyn InputPrompter + Send + Sync>) {
        self.input_handler = Some(handler);
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

    /// Resolves `identifier` (or dispatches directly to the registered
    /// [`InputPrompter`] for the built-in
    /// `"$input"` identifier), looks up the target service and its
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
        // SimpleCode -> CodeRunner
        // ApiWrapper -> FilteredRunner
        // ScriptedAction -> ScriptRunner

        if identifier == "$input" {
            if let &Some(ref input_handler) = &self.input_handler {
                return input_handler.run(params, context);
            }

            return Err(error::ExecutionEngine::Unimplemented(
                "Input Handler".into(),
            ));
        }

        let (service_name, operation_name) =
            Self::parse_identifier(identifier, context.parent.as_deref())?;

        let (service, credentials) = {
            let lookup = self
                .lookup
                .lock()
                .map_err(|err| error::ExecutionEngine::PoisonedLock(err.to_string()))?;
            let service = lookup
                .get_service(service_name)
                .ok_or_else(|| error::ExecutionEngine::NotFound(identifier.into()))?;

            let credentials = lookup.get_credentials(service_name);

            (service, credentials)
        };
        let service = service.v1();
        let manifest = service.manifest.v2();

        let result = match &manifest.value {
            &Some(service_manifest_latest::Value::Swagger(ref swagger)) => {
                if let &Some(ref connector) = &self.connector {
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
            &Some(service_manifest_latest::Value::Action(ref action)) => {
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
            &Some(service_manifest_latest::Value::ApiWrapped(ref api_wrapped)) => {
                if let &Some(ref filtered_runner) = &self.filtered_runner {
                    self.log(identifier, "API_WRAPPED", "STARTED")?;
                    let result = filtered_runner.run(
                        service_name,
                        operation_name,
                        api_wrapped,
                        params,
                        context,
                    )?;
                    self.log(identifier, "API_WRAPPED", "COMPLETED")?;

                    Ok(result)
                } else {
                    Err(error::ExecutionEngine::NotFound(
                        "API Wrapper runner not found".into(),
                    ))
                }
            }
            &Some(service_manifest_latest::Value::SimpleCode(ref simple_code)) => {
                match simple_code.code.language.enum_value() {
                    Ok(Language::PYTHON) => {
                        if let Some(code_runner) = self.code_runners.get("python") {
                            self.log(identifier, "SIMPLE_CODE", "STARTED")?;
                            let result = code_runner.run(
                                service_name,
                                operation_name,
                                simple_code.code.codeString(),
                                params,
                                context,
                            )?;
                            self.log(identifier, "SIMPLE_CODE", "COMPLETED")?;

                            Ok(result)
                        } else {
                            Err(error::ExecutionEngine::NotFound(
                                "Code runner not found for python".into(),
                            ))
                        }
                    }
                    Ok(Language::JAVASCRIPT) => {
                        if let Some(code_runner) = self.code_runners.get("js") {
                            self.log(identifier, "SIMPLE_CODE", "STARTED")?;
                            let result = code_runner.run(
                                service_name,
                                operation_name,
                                simple_code.code.codeString(),
                                params,
                                context,
                            )?;
                            self.log(identifier, "SIMPLE_CODE", "COMPLETED")?;

                            Ok(result)
                        } else {
                            Err(error::ExecutionEngine::NotFound(
                                "Code runner not found for python".into(),
                            ))
                        }
                    }
                    _ => Err(error::ExecutionEngine::NotFound("Unknown language".into())),
                }
            }
            _ => Err(error::ExecutionEngine::Unimplemented("API Runner".into())),
        }?;

        if context.raw_response {
            Ok(result)
        } else if let Value::Array(_) = result {
            Ok(result)
        } else {
            Ok(Value::Array(vec![result]))
        }
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

        let lookup: Arc<Mutex<dyn EngineLookup + Send + Sync>> = Arc::new(Mutex::new(FakeLookup));
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
}
