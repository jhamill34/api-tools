//!

use credential_entities::credentials::Authentication;
use serde_json::Value;
use core_entities::service::{
    APIWrappedService, CommonApi, ScriptedAction, SwaggerService, VersionedServiceTree,
};

use crate::error;

///
#[non_exhaustive]
pub struct EngineInputContext {
    ///
    pub parent: Option<String>,

    ///
    pub execution_id: String,

    ///
    pub raw_response: bool,
}

impl EngineInputContext {
    ///
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

///
pub trait EngineLookup {
    ///
    fn get_service(&self, id: &str) -> Option<VersionedServiceTree>;

    ///
    fn get_credentials(&self, id: &str) -> Option<Authentication>;
}

///
pub trait InputPrompter {
    ///
    /// # Errors
    fn run(&self, params: Value, ctx: &EngineInputContext) -> error::Result<Value>;
}

///
#[non_exhaustive]
pub struct DataConnectorBundle<'bundle> {
    ///
    pub manifest: &'bundle SwaggerService,

    ///
    pub api: &'bundle CommonApi,

    ///
    pub creds: Option<&'bundle Authentication>,
}

///
pub trait DataConnectionRunner {
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

///
pub trait CodeRunner {
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

///
pub trait FilteredRunner {
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

///
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
