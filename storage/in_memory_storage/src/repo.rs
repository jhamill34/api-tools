//! The [`Repository`] storage port and its in-memory implementation.

use super::error;

extern crate alloc;
use alloc::collections::BTreeMap;

/// A keyed store of values of type `V`.
pub trait Repository<V> {
    /// Lists the IDs of every stored value.
    fn list(&self) -> Vec<String>;

    /// Looks up the value stored under `id`, if any.
    fn get(&self, id: &str) -> Option<V>;

    /// Stores `value` under `id`, overwriting any value already there.
    ///
    /// # Errors
    fn save(&mut self, id: String, value: V) -> Result<(), error::OperationRepo>;

    /// Removes the value stored under `id`, if any.
    ///
    /// # Errors
    fn remove(&mut self, id: &str) -> Result<(), error::OperationRepo>;
}

/// This below could be a different crate...
pub struct InMemoryRepository<V> {
    /// The underlying key-value store.
    storage: BTreeMap<String, V>,
}

impl<V> InMemoryRepository<V> {
    /// Creates an empty [`InMemoryRepository`].
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self {
            storage: BTreeMap::new(),
        }
    }
}

impl<V> Default for InMemoryRepository<V> {
    /// Creates an empty [`InMemoryRepository`], same as [`new`](InMemoryRepository::new).
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
