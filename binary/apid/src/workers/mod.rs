//! Background threads that watch loaded services' directories for changes
//! and hot-reload them into the shared [`LocalDirectoryCatalog`].

mod loader;
mod watcher;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{mpsc, Arc},
    thread::JoinHandle,
};

use local_directory_catalog::LocalDirectoryCatalog;

/// Starts the file-watcher and loader background threads (see
/// [`watcher`] and [`loader`]) and kicks off an initial load of every
/// service in `paths`.
pub fn start_background_watcher(
    catalog: Arc<LocalDirectoryCatalog>,
    paths: &Arc<HashMap<String, PathBuf>>,
) -> anyhow::Result<(JoinHandle<()>, JoinHandle<()>)> {
    let (file_tx, file_rx) = mpsc::channel::<Vec<String>>();
    let (load_tx, load_rx) = mpsc::channel::<bool>();

    let watcher_handler = watcher::start(Arc::clone(paths), file_tx.clone(), load_rx);
    let loading_handler = loader::start(catalog, load_tx, file_rx);

    let all_services: Vec<_> = paths.keys().cloned().collect();
    file_tx.send(all_services)?;

    Ok((watcher_handler, loading_handler))
}
