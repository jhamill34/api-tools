//!

mod loader;
mod watcher;

extern crate alloc;
use alloc::sync::Arc;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{mpsc, Mutex},
    thread::JoinHandle,
};

use in_memory_storage::OperationRepos;

///
pub fn start_background_watcher(
    repos: Arc<Mutex<OperationRepos>>,
    paths: &Arc<HashMap<String, PathBuf>>,
) -> anyhow::Result<(JoinHandle<()>, JoinHandle<()>)> {
    let (file_tx, file_rx) = mpsc::channel::<Vec<String>>();
    let (load_tx, load_rx) = mpsc::channel::<bool>();

    let watcher_handler = watcher::start(Arc::clone(paths), file_tx.clone(), load_rx);
    let loading_handler = loader::start(repos, Arc::clone(paths), load_tx, file_rx);

    let all_services: Vec<_> = paths.keys().cloned().collect();
    file_tx.send(all_services)?;

    Ok((watcher_handler, loading_handler))
}
