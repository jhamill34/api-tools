//!

use super::error;

extern crate alloc;
use alloc::collections::BTreeMap;

///
pub trait Repository<V> {
    ///
    fn list(&self) -> Vec<String>;

    ///
    fn get(&self, id: &str) -> Option<V>;

    ///
    /// # Errors
    fn save(&mut self, id: String, value: V) -> Result<(), error::OperationRepo>;

    /// # Errors
    fn remove(&mut self, id: &str) -> Result<(), error::OperationRepo>;
}

/// This below could be a different crate...
pub struct InMemoryRepository<V> {
    ///
    storage: BTreeMap<String, V>,
}

impl<V> InMemoryRepository<V> {
    ///
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self {
            storage: BTreeMap::new(),
        }
    }
}

impl<V> Default for InMemoryRepository<V> {
    ///
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<V: Clone> Repository<V> for InMemoryRepository<V> {
    #[inline]
    fn list(&self) -> Vec<String> {
        self.storage
            .keys()
            .map(alloc::borrow::ToOwned::to_owned)
            .collect()
    }

    #[inline]
    fn get(&self, id: &str) -> Option<V> {
        self.storage.get(id).cloned()
    }

    #[inline]
    fn save(&mut self, id: String, value: V) -> Result<(), error::OperationRepo> {
        self.storage.insert(id, value);
        Ok(())
    }

    #[inline]
    fn remove(&mut self, id: &str) -> Result<(), error::OperationRepo> {
        self.storage.remove(id);
        Ok(())
    }
}
