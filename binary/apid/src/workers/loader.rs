//! The background thread that (re)loads changed services into the shared
//! [`LocalDirectoryCatalog`], signalled by [`super::watcher`].

use std::{
    sync::{
        mpsc::{Receiver, Sender},
        Arc,
    },
    thread::{self, JoinHandle},
};

use local_directory_catalog::LocalDirectoryCatalog;
use tracing::{error, info};

/// Spawns a thread that waits on `rx` for batches of changed service names,
/// re-reads each one from disk into `catalog`, then signals readiness on
/// `tx` (both at startup and after each reload batch) so the watcher thread
/// knows it's safe to report the next batch of changes.
pub fn start(
    catalog: Arc<LocalDirectoryCatalog>,
    tx: Sender<bool>,
    rx: Receiver<Vec<String>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        if let Err(err) = tx.send(true) {
            error!(%err, "unable to signal to watcher thread ready");
            return;
        }

        for event in rx {
            for service in event {
                if let Err(err) = catalog.refresh(&service) {
                    error!(%err, %service, "error loading service");
                } else {
                    info!(%service, "reloading service");
                }
            }

            if let Err(err) = tx.send(true) {
                error!(%err, "unable to signal to watcher thread ready");
                return;
            }
        }
    })
}
