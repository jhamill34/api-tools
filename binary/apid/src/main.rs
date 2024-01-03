#![warn(clippy::restriction, clippy::pedantic)]
#![allow(
    clippy::blanket_clippy_restriction_lints,
    clippy::mod_module_files,
    clippy::self_named_module_files,
    clippy::implicit_return,
    clippy::shadow_reuse,
    clippy::match_ref_pats,
    clippy::shadow_unrelated,
    clippy::shadow_same,
    // clippy::too_many_lines
    clippy::question_mark_used,
)]

//!

mod config;
mod constants;
mod util;
mod workers;

extern crate alloc;
use alloc::sync::Arc;
use config::Configuration;

use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    path::PathBuf,
    sync::{mpsc::Sender, Mutex, PoisonError, RwLock},
    thread,
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
    ListResponse, ProvideInputRequest, ProvideInputResponse, RunServiceRequest, RunServiceResponse,
    SaveServiceRequest, SaveServiceResponse,
};
use execution_engine::services::EngineLookup;
use in_memory_storage::{repo::InMemoryRepository, OperationRepos};
use local_file_loader::LocalFileFetcher;
use protobuf::Message;
use service_writer::ServiceWriter;
use tonic::{transport::Server, Request, Response, Status};
use user_input::Signals;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

///
struct ApiDaemon {
    ///
    repos: Arc<Mutex<OperationRepos>>,

    ///
    paths: Arc<HashMap<String, PathBuf>>,

    ///
    engine: Arc<RwLock<execution_engine::Engine>>,

    ///
    responses: Arc<Mutex<HashMap<String, GetRunResultResponse>>>,

    ///
    signals: Signals,
}

impl ApiDaemon {
    ///
    #[must_use]
    #[inline]
    fn new(
        repos: Arc<Mutex<OperationRepos>>,
        paths: Arc<HashMap<String, PathBuf>>,
        engine: Arc<RwLock<execution_engine::Engine>>,
        responses: Arc<Mutex<HashMap<String, GetRunResultResponse>>>,
        signals: Signals,
    ) -> Self {
        Self {
            repos,
            paths,
            engine,
            responses,
            signals,
        }
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

        let writer = ServiceWriter::default();

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
        let signals = Arc::clone(&self.signals);

        // TODO: convert to using a ThreadPool
        thread::spawn(move || {
            let ctx = execution_engine::services::EngineInputContext::new(
                None,
                execution_id.to_string(),
                false,
            );
            let engine = engine.read().unwrap_or_else(PoisonError::into_inner);

            // TODO: Better error handling, Engine::run should NOT panic!
            let result = engine.run(&req.id, input, options, &ctx);

            let execution_id = execution_id.to_string();
            match result {
                Ok(result) => match serde_json::to_string_pretty(&result) {
                    Ok(result) => {
                        let result = GetRunResultResponse {
                            status: get_run_result_response::Status::Completed.into(),
                            output: Some(result),
                        };

                        let mut responses =
                            responses.lock().unwrap_or_else(PoisonError::into_inner);
                        responses.insert(execution_id.clone(), result);
                    }
                    Err(err) => {
                        let result = GetRunResultResponse {
                            status: get_run_result_response::Status::Completed.into(),
                            output: Some(format!("{{ \"error\": \"{err}\" }}")),
                        };

                        let mut responses =
                            responses.lock().unwrap_or_else(PoisonError::into_inner);
                        responses.insert(execution_id.clone(), result);
                    }
                },
                Err(err) => {
                    let result = GetRunResultResponse {
                        status: get_run_result_response::Status::Completed.into(),
                        output: Some(format!("{{ \"error\": \"{err}\" }}")),
                    };

                    let mut responses = responses.lock().unwrap_or_else(PoisonError::into_inner);
                    responses.insert(execution_id.clone(), result);
                }
            };

            let mut signals = signals.lock().unwrap_or_else(PoisonError::into_inner);
            signals.remove(&execution_id);
        });

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

        if result.status() == get_run_result_response::Status::Running {
            let signals = self.signals.lock().unwrap_or_else(PoisonError::into_inner);

            if let Some(response) = signals.get(&req.execution_id) {
                match serde_json::to_string_pretty(&response.0) {
                    Ok(output) => {
                        return Ok(Response::new(GetRunResultResponse {
                            status: get_run_result_response::Status::Waiting.into(),
                            output: Some(output),
                        }))
                    }
                    Err(err) => {
                        return Ok(Response::new(GetRunResultResponse {
                            status: get_run_result_response::Status::Waiting.into(),
                            output: Some(err.to_string()),
                        }))
                    }
                }
            }
        }

        Ok(Response::new(result))
    }

    async fn provide_input(
        &self,
        req: Request<ProvideInputRequest>,
    ) -> Result<Response<ProvideInputResponse>, Status> {
        let req = req.into_inner();

        let mut signals = self.signals.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(&mut (_, ref tx)) = signals.get_mut(&req.execution_id) {
            let value = serde_json::from_str::<serde_json::Value>(&req.input);
            if let Ok(value) = value {
                tx.send(value).map_err(|e| {
                    Status::data_loss(format!("Unable to send user input for this execution: {e}"))
                })?;
            }
        }

        Ok(Response::new(ProvideInputResponse {}))
    }
}

///
fn construct_execution_engine(
    lookup: Arc<Mutex<dyn EngineLookup + Sync + Send>>,
    signals: Signals,
    config: &Configuration,
) -> anyhow::Result<Arc<RwLock<execution_engine::Engine>>> {
    let workflow_logger = Arc::new(RwLock::new(File::create(config.log.workflow_path.clone())?));

    let api_logger = Arc::new(RwLock::new(File::create(config.log.api_path.clone())?));

    let engine = Arc::new(RwLock::new(execution_engine::Engine::new(
        lookup,
        Arc::clone(&workflow_logger),
    )));

    let connector = Box::new(api_caller::APICaller::new(api_logger));

    #[cfg(feature = "python")]
    let py_runner =
        python_runner::PyActionRunner::new(Arc::clone(&workflow_logger), Arc::clone(&engine));

    #[cfg(feature = "javascript")]
    let js_runner =
        javascript_runner::JsActionRunner::new(Arc::clone(&engine), Arc::clone(&workflow_logger));

    #[cfg(feature = "input")]
    let input_handler = Box::new(user_input::UserInput::new(signals));

    #[cfg(feature = "wrapper")]
    let api_wrapper = filtered_runner::APIWrapper::new(workflow_logger, Arc::clone(&engine));

    {
        let mut engine = engine
            .write()
            .map_err(|e| anyhow!("Unable to setup execution engine...: {e}"))?;
        engine.register_connector(connector);

        #[cfg(feature = "python")]
        engine.register_language(constants::PYTHON_LANG, Box::new(py_runner));

        #[cfg(feature = "javascript")]
        engine.register_language(constants::JAVASCRIPT_LANG, Box::new(js_runner));

        #[cfg(feature = "input")]
        engine.register_input(input_handler);

        #[cfg(feature = "wrapper")]
        engine.register_filtered_runner(Box::new(api_wrapper));
    };

    Ok(engine)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
    let signals = HashMap::<String, (serde_json::Value, Sender<serde_json::Value>)>::new();
    let signals = Arc::new(Mutex::new(signals));

    let engine = construct_execution_engine(
        Arc::<Mutex<in_memory_storage::OperationRepos>>::clone(&repos),
        Arc::clone(&signals),
        &config,
    )?;

    // Start Server
    // println!("Starting server...");

    let engine = ApiDaemon::new(repos, paths, engine, response_store, signals);
    let addr = format!("{}:{}", config.server.host, config.server.port).parse()?;
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
