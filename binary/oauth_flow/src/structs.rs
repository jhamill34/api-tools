use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use core_entities::service::VersionedServiceTree;
use credential_entities::credentials::Authentication;

pub struct EnvironmentState {
    pub service: VersionedServiceTree,
    pub creds: Arc<Mutex<Authentication>>,
    pub redirect_uri: String,
}

impl EnvironmentState {
    /// Locks the shared credentials, recovering from a poisoned lock instead
    /// of panicking (consistent with binary/apid's lock-handling pattern for
    /// the same kind of shared state).
    pub fn lock_creds(&self) -> MutexGuard<'_, Authentication> {
        self.creds.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[test]
    fn lock_creds_recovers_from_a_poisoned_lock() {
        let env = EnvironmentState {
            service: VersionedServiceTree::default(),
            creds: Arc::new(Mutex::new(Authentication::Header(
                credential_entities::credentials::HeaderCredentials::default(),
            ))),
            redirect_uri: String::new(),
        };

        // Poison the mutex by panicking on another thread while holding the lock.
        let creds = Arc::clone(&env.creds);
        let _ = thread::spawn(move || {
            let _guard = creds.lock().unwrap();
            panic!("intentionally poisoning the mutex for the test");
        })
        .join();

        // Must not panic even though the lock is now poisoned.
        let _guard = env.lock_creds();
    }
}
