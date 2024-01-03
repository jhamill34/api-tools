#![allow(clippy::print_stdout, clippy::use_debug)]

//!

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

///
pub fn start(
    repos: Arc<Mutex<OperationRepos>>,
    paths: Arc<HashMap<String, PathBuf>>,
    tx: Sender<bool>,
    rx: Receiver<Vec<String>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let loader = ServiceLoader::default();

        if let Err(err) = tx.send(true) {
            println!("Unable to signal to watcher thread ready: {err}");
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
                        println!("Error loading {service}:\n{err:?}");
                    } else {
                        println!("Reloading {service}");
                    }
                } else {
                    println!("Service not found?: {service}");
                }
            }

            if let Err(err) = tx.send(true) {
                println!("Unable to signal to watcher thread ready: {err}");
                return;
            }
        }
    })
}
