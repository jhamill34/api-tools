//! An in-memory storage adapter that backs both a service loader's
//! [`LoaderOutput`] (persisting loaded services/credentials) and an
//! execution engine's [`EngineLookup`] (resolving them again at execution
//! time).

pub mod error;
pub mod repo;

use core_entities::entity::VersionedServiceTree;
use core_entities::ports::{engine::EngineLookup, loader::LoaderOutput};
use credential_entities::entity::Authentication;
use repo::Repository;

/// Bundles the two [`Repository`] instances a loaded workspace needs: one
/// for service manifests, one for credentials. Implements both
/// [`LoaderOutput`] and [`EngineLookup`] over the same underlying storage.
#[non_exhaustive]
pub struct OperationRepos {
    /// Backing store for loaded service manifests, keyed by service ID.
    pub services: Box<dyn Repository<VersionedServiceTree> + Send + Sync>,

    /// Backing store for loaded credentials, keyed by credential ID.
    pub credentials: Box<dyn Repository<Authentication> + Send + Sync>,
}

impl OperationRepos {
    /// Bundles the given `services` and `credentials` repositories.
    #[inline]
    #[must_use]
    pub fn new(
        services: Box<dyn Repository<VersionedServiceTree> + Send + Sync>,
        credentials: Box<dyn Repository<Authentication> + Send + Sync>,
    ) -> Self {
        Self {
            services,
            credentials,
        }
    }
}

impl LoaderOutput for OperationRepos {
    #[inline]
    fn handle_service(
        &mut self,
        id: &str,
        service: VersionedServiceTree,
    ) -> core_entities::ports::loader::Result<()> {
        self.services.save(id.to_owned(), service)?;
        Ok(())
    }

    #[inline]
    fn handle_credentials(
        &mut self,
        id: &str,
        credentials: Authentication,
    ) -> core_entities::ports::loader::Result<()> {
        self.credentials.save(id.to_owned(), credentials)?;
        Ok(())
    }
}

impl EngineLookup for OperationRepos {
    #[inline]
    fn get_service(&self, id: &str) -> Option<VersionedServiceTree> {
        self.services.get(id)
    }

    #[inline]
    fn get_credentials(&self, id: &str) -> Option<Authentication> {
        self.credentials.get(id)
    }
}
