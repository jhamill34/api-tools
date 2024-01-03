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

//! Crate Docs

pub mod error;
pub mod services;

///
mod constants;

extern crate alloc;
use alloc::sync::Arc;

use serde_json::Value;
use services::{
    CodeRunner, DataConnectionRunner, DataConnectorBundle, EngineInputContext, EngineLookup,
    FilteredRunner, InputPrompter, ScriptRunner,
};
use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    sync::{Mutex, RwLock},
};

use chrono::offset::Local;
use core_entities::service::{code_resource::Language, service_manifest_latest};

///
pub struct Engine {
    ///
    lookup: Arc<Mutex<dyn EngineLookup + Send + Sync>>,

    ///
    logger: Arc<RwLock<File>>,

    ///
    connector: Option<Box<dyn DataConnectionRunner + Send + Sync>>,

    ///
    code_runners: HashMap<String, Box<dyn CodeRunner + Send + Sync>>,

    ///
    script_runner: Option<Box<dyn ScriptRunner + Send + Sync>>,

    ///
    filtered_runner: Option<Box<dyn FilteredRunner + Send + Sync>>,

    ///
    input_handler: Option<Box<dyn InputPrompter + Send + Sync>>,
}

impl Engine {
    ///
    #[inline]
    pub fn new(
        lookup: Arc<Mutex<dyn EngineLookup + Send + Sync>>,
        logger: Arc<RwLock<File>>,
    ) -> Self {
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

    ///
    #[inline]
    pub fn register_language(&mut self, lang: &str, runner: Box<dyn CodeRunner + Send + Sync>) {
        self.code_runners.insert(lang.to_owned(), runner);
    }

    ///
    #[inline]
    pub fn register_script_runner(&mut self, runner: Box<dyn ScriptRunner + Send + Sync>) {
        self.script_runner = Some(runner);
    }

    ///
    #[inline]
    pub fn register_filtered_runner(&mut self, runner: Box<dyn FilteredRunner + Send + Sync>) {
        self.filtered_runner = Some(runner);
    }

    ///
    #[inline]
    pub fn register_connector(&mut self, runner: Box<dyn DataConnectionRunner + Send + Sync>) {
        self.connector = Some(runner);
    }

    ///
    #[inline]
    pub fn register_input(&mut self, handler: Box<dyn InputPrompter + Send + Sync>) {
        self.input_handler = Some(handler);
    }

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

        let parts: Vec<&str> = identifier.split('.').collect();

        let service_name = parts
            .first()
            .ok_or_else(|| error::ExecutionEngine::InvalidIdentifier(identifier.into()))?;
        let operation_name = parts
            .get(1)
            .ok_or_else(|| error::ExecutionEngine::InvalidIdentifier(identifier.into()))?;

        let service_name = match &context.parent {
            &Some(ref parent) if *service_name == "this" => parent,
            _ => *service_name,
        };

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

    ///
    fn log(&self, id: &str, action_type: &str, status: &str) -> error::Result<()> {
        let now = Local::now();
        let now = now.format(constants::DATETIME_FORMAT).to_string();

        let mut logger = self
            .logger
            .write()
            .map_err(|err| error::ExecutionEngine::PoisonedLock(err.to_string()))?;
        logger.write_all(format!("{now} ({action_type}) [{status}] {id}\n").as_bytes())?;

        Ok(())
    }
}
