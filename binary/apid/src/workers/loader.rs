//! The background thread that (re)loads changed services into the shared
//! repositories, signalled by [`super::watcher`].

extern crate alloc;
use alloc::sync::Arc;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        mpsc::{Receiver, Sender},
        Mutex, PoisonError,
    },
    thread::{self, JoinHandle},
};

use in_memory_storage::OperationRepos;
use local_file_loader::LocalFileFetcher;
use service_loader::ServiceLoader;
use tracing::{error, info, warn};

/// Spawns a thread that waits on `rx` for batches of changed service names,
/// reloads each one from `paths` into `repos`, then signals readiness on
/// `tx` (both at startup and after each reload batch) so the watcher thread
/// knows it's safe to report the next batch of changes.
pub fn start(
    repos: Arc<Mutex<OperationRepos>>,
    paths: Arc<HashMap<String, PathBuf>>,
    tx: Sender<bool>,
    rx: Receiver<Vec<String>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let loader = ServiceLoader::default();

        if let Err(err) = tx.send(true) {
            error!(%err, "unable to signal to watcher thread ready");
            return;
        }

        for event in rx {
            let mut repos = repos.lock().unwrap_or_else(PoisonError::into_inner);
            let repos = &mut *repos;
            for service in event {
                if let Some(path) = paths.get(&service) {
                    let fetcher = LocalFileFetcher::from(path.clone());
                    if let Err(err) = loader
                        .load(&service, &fetcher, repos, true, false)
                        .map_err(anyhow::Error::from)
                    {
                        error!(?err, %service, "error loading service");
                    } else {
                        info!(%service, "reloading service");
                    }
                } else {
                    warn!(%service, "service not found");
                }
            }

            if let Err(err) = tx.send(true) {
                error!(%err, "unable to signal to watcher thread ready");
                return;
            }
        }
    })
}
