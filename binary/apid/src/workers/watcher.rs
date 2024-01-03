#![allow(clippy::print_stdout)]

//!

extern crate alloc;
use alloc::sync::Arc;

use core::time::Duration;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex, PoisonError,
    },
    thread::{self, JoinHandle},
};

use notify::Watcher;

///
pub fn start(
    paths: Arc<HashMap<String, PathBuf>>,
    tx: Sender<Vec<String>>,
    rx: Receiver<bool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut watchers = vec![];
        let cache = Arc::new(Mutex::new(HashSet::<String>::new()));
        let (inner_tx, inner_rx) = mpsc::channel::<bool>();

        for (name, path) in &*paths {
            let config = notify::Config::default()
                .with_poll_interval(Duration::from_secs(1))
                .with_compare_contents(false);

            let cache = Arc::clone(&cache);
            let name = name.clone();
            let inner_tx = inner_tx.clone();
            let watcher = notify::PollWatcher::new(
                move |_| {
                    let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
                    cache.insert(name.clone());

                    match inner_tx.send(true) {
                        Ok(_) => {}
                        Err(err) => println!("Unable to signal read for loading: {err}"),
                    }
                },
                config,
            );

            match watcher {
                Ok(mut watcher) => match watcher.watch(path, notify::RecursiveMode::Recursive) {
                    Ok(()) => {
                        println!("Started watcher for: {}", path.to_string_lossy());
                        watchers.push(watcher);
                    }
                    Err(err) => {
                        println!("Unable start PollWatcher: {err}");
                    }
                },
                Err(err) => {
                    println!("Unable create PollWatcher: {err}");
                }
            }
        }

        for is_ready in rx {
            if is_ready {
                for has_updates in &inner_rx {
                    if has_updates {
                        // Sleep to catch any potential double updates
                        // At least one polling iteration
                        thread::sleep(Duration::from_secs(2));

                        let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
                        if let Err(err) = tx.send(cache.drain().collect()) {
                            println!("Unable to signal to loader thread what to load: {err}");
                            return;
                        }
                    }
                }
            }
        }
    })
}
