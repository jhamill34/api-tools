//! The daemon binary: bootstraps concrete adapters into an
//! [`execution_engine::Engine`] and starts the [`grpc_api`] gRPC server
//! over them, the composition root of the whole workspace.

mod config;
mod constants;
mod util;
mod workers;

use config::Configuration;

use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    path::PathBuf,
    sync::Arc,
};

use anyhow::{anyhow, Context};
use core_entities::ports::{
    catalog::{ServiceCatalog, ServiceCatalogWriter},
    engine::EngineService,
};
use dotenv::dotenv;
use local_directory_catalog::LocalDirectoryCatalog;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// Builds an [`execution_engine::Engine`] backed by `lookup`, and registers
/// every adapter enabled by this build's Cargo features (the API-call
/// connector is always registered; Python/JavaScript/Lua code runners and
/// the filtered-runner wrapper are each gated behind their own feature
/// flag — `lua` isn't in this build's `default` set yet).
///
/// Does blocking work (`reqwest::blocking::Client::new()` internally
/// spins up its own tokio runtime, which cannot be constructed or torn
/// down from an already-running async context) — callers on the async
/// main thread must invoke this through `tokio::task::spawn_blocking`.
fn construct_execution_engine(
    lookup: Arc<dyn ServiceCatalog + Sync + Send>,
    workflow_path: &str,
    api_path: &str,
) -> anyhow::Result<Arc<dyn EngineService>> {
    let (workflow_logger, _workflow_logger_handle) =
        common_data_structures::log_writer::LogWriter::spawn(File::create(workflow_path)?);

    let (api_logger, _api_logger_handle) =
        common_data_structures::log_writer::LogWriter::spawn(File::create(api_path)?);

    // Every adapter below gets a handle to the engine before it's finished
    // being registered onto it below - `Arc::new_cyclic` hands us a
    // `Weak<Engine>` that's valid to clone into those adapters right now,
    // and becomes upgradable the instant this closure returns and the real
    // `Arc<Engine>` exists. See `WeakEngine`'s docs for why a non-owning
    // `Weak` handle (rather than a strong `Arc`) is what avoids a reference
    // cycle here: `Engine` owns each adapter, so a strong handle back to
    // `Engine` from inside an adapter it owns would be a cycle neither side
    // could ever be freed from.
    let engine = Arc::new_cyclic(|weak_engine| {
        let weak_handle: Arc<dyn EngineService> =
            Arc::new(execution_engine::WeakEngine::new(weak_engine.clone()));

        let mut engine = execution_engine::Engine::new(lookup, workflow_logger.clone());

        let connector = Box::new(api_caller::APICaller::new(api_logger.clone()));

        #[cfg(feature = "python")]
        let py_runner =
            python_runner::PyActionRunner::new(workflow_logger.clone(), Arc::clone(&weak_handle));

        #[cfg(feature = "javascript")]
        let js_runner = javascript_runner::JsActionRunner::new(
            Arc::clone(&weak_handle),
            workflow_logger.clone(),
        );

        // `pool_size` has no config field yet, so this gets
        // `workflow_runner`'s own default pool size for now - see #103.
        #[cfg(feature = "workflow")]
        let workflow_adapter =
            workflow_runner::WorkflowAdapter::spawn(&weak_handle, &workflow_logger, None);

        #[cfg(feature = "workflow")]
        let async_connector = Arc::new(api_caller::AsyncAPICaller::new(api_logger));

        #[cfg(feature = "wrapper")]
        let api_wrapper =
            filtered_runner::APIWrapper::new(workflow_logger.clone(), Arc::clone(&weak_handle));

        engine.register_connector(connector);

        #[cfg(feature = "python")]
        engine.register_language(constants::PYTHON_LANG, Box::new(py_runner));

        #[cfg(feature = "javascript")]
        engine.register_language(constants::JAVASCRIPT_LANG, Box::new(js_runner));

        #[cfg(feature = "workflow")]
        engine.register_workflow_runner(Arc::new(workflow_adapter));

        #[cfg(feature = "workflow")]
        engine.register_async_connector(async_connector);

        #[cfg(feature = "wrapper")]
        engine.register_filtered_runner(Box::new(api_wrapper));

        engine
    });

    let engine: Arc<dyn EngineService> = engine;
    Ok(engine)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_err| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    #[cfg(feature = "dhat-ad-hoc")]
    let _profiler = dhat::Profiler::new_ad_hoc();

    dotenv().ok();

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
    let catalog = Arc::new(LocalDirectoryCatalog::new(paths.clone()));
    let paths = Arc::new(paths);

    // Spawn off our background loader
    let (watcher_handler, loader_handler) =
        workers::start_background_watcher(Arc::clone(&catalog), &paths)?;

    let engine = {
        let lookup: Arc<dyn ServiceCatalog + Sync + Send> = catalog.clone();
        let workflow_path = config.log.workflow_path.clone();
        let api_path = config.log.api_path.clone();

        tokio::task::spawn_blocking(move || {
            construct_execution_engine(lookup, &workflow_path, &api_path)
        })
        .await??
    };

    let catalog_writer: Arc<dyn ServiceCatalogWriter + Send + Sync> = catalog.clone();
    let catalog: Arc<dyn ServiceCatalog + Send + Sync> = catalog;
    let addr = format!("{}:{}", config.server.host, config.server.port).parse()?;

    tracing::info!(%addr, "starting server");

    grpc_api::serve(catalog, catalog_writer, engine, addr).await?;

    loader_handler
        .join()
        .map_err(|_e| anyhow!("Panic occurred in loader handler"))?;
    watcher_handler
        .join()
        .map_err(|_e| anyhow!("Panic occured in watcher handler"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn construct_execution_engine_does_not_panic_when_called_via_spawn_blocking() {
        let catalog = LocalDirectoryCatalog::new(HashMap::new());
        let catalog: Arc<dyn ServiceCatalog + Sync + Send> = Arc::new(catalog);

        let log_dir = tempfile::tempdir().unwrap();
        let workflow_path = log_dir
            .path()
            .join("workflow.log")
            .to_string_lossy()
            .into_owned();
        let api_path = log_dir
            .path()
            .join("api.log")
            .to_string_lossy()
            .into_owned();

        // Mirrors exactly how main() calls this: from the async runtime,
        // but through spawn_blocking rather than directly — calling it
        // directly here would reproduce the panic this test guards
        // against (reqwest::blocking::Client::new() cannot construct its
        // own tokio runtime from within an already-running one).
        let result = tokio::task::spawn_blocking(move || {
            construct_execution_engine(catalog, &workflow_path, &api_path)
        })
        .await;

        assert!(
            result.is_ok(),
            "expected the spawn_blocking task itself not to panic, got {:?}",
            result.as_ref().err()
        );
        assert!(
            result.unwrap().is_ok(),
            "expected construct_execution_engine to succeed"
        );
    }
}
