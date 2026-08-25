#![warn(clippy::restriction, clippy::pedantic)]
#![allow(
    clippy::blanket_clippy_restriction_lints,
    clippy::mod_module_files,
    clippy::self_named_module_files,

    clippy::implicit_return,
    clippy::shadow_reuse,
    clippy::match_ref_pats,

    // Would like to turn on (Configured to 50?)
    clippy::too_many_lines,
    clippy::question_mark_used,
    clippy::needless_borrowed_reference,
    clippy::absolute_paths,
    clippy::ref_patterns,
    clippy::single_call_fn
)]

//! An in-memory storage adapter that backs both [`service_loader`]'s
//! [`LoaderOutput`] (persisting loaded services/credentials) and
//! [`execution_engine`]'s [`EngineLookup`] (resolving them again at
//! execution time).

pub mod error;
pub mod repo;

use core_entities::service::VersionedServiceTree;
use credential_entities::credentials::Authentication;
use execution_engine::services::EngineLookup;
use repo::Repository;
use service_loader::LoaderOutput;

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
    ) -> service_loader::error::Result<()> {
        self.services.save(id.to_owned(), service)?;
        Ok(())
    }

    #[inline]
    fn handle_credentials(
        &mut self,
        id: &str,
        credentials: Authentication,
    ) -> service_loader::error::Result<()> {
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
