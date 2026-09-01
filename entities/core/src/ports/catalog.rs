//! The [`ServiceCatalog`] and [`ServiceCatalogWriter`] ports: read and write
//! access to the loaded service catalog, for any driving adapter that reads
//! or edits it - `apid`'s gRPC `list`/`get_service`/`save_service` handlers,
//! and the execution engine's own runtime lookups (`ServiceCatalogWriter`'s
//! methods are never in scope for the engine, since it's only ever handed
//! a `&dyn ServiceCatalog`).

use crate::service::VersionedServiceTree;
use credential_entities::credentials::Authentication;

/// Errors produced while reading or writing the loaded service catalog.
pub mod error {
    use std::io;

    use thiserror::Error;

    /// Failure modes of a [`ServiceCatalogWriter`](super::ServiceCatalogWriter)
    /// implementation - deliberately a small, port-contract-shaped enum, not
    /// whichever concrete writer/storage crate an adapter happens to be
    /// built on.
    #[derive(Debug, Error)]
    #[non_exhaustive]
    pub enum CatalogError {
        /// A required piece of the input, such as where to write a named
        /// service, was missing.
        #[error("Not found: {0}")]
        NotFound(String),

        /// Writing to or reading from the underlying storage failed.
        #[error(transparent)]
        Io {
            /// The underlying I/O error.
            #[from]
            source: io::Error,
        },

        /// Some other adapter-specific failure.
        #[error(transparent)]
        Other {
            /// The wrapped error from the [`ServiceCatalogWriter`](super::ServiceCatalogWriter)
            /// implementation.
            source: anyhow::Error,
        },
    }

    /// Shorthand for a [`Result`](core::result::Result) using
    /// [`CatalogError`] as its error type.
    pub type Result<T> = core::result::Result<T, CatalogError>;
}

/// An input port a driving adapter reads the loaded service catalog from.
pub trait ServiceCatalog {
    /// Lists the IDs of every loaded service.
    fn list(&self) -> Vec<String>;

    /// Looks up a loaded service manifest by ID.
    fn get_service(&self, id: &str) -> Option<VersionedServiceTree>;

    /// Looks up loaded credentials by ID.
    fn get_credentials(&self, id: &str) -> Option<Authentication>;
}

/// An input port a driving adapter writes to the loaded service catalog
/// through.
pub trait ServiceCatalogWriter {
    /// Persists `service` under `id`.
    ///
    /// # Errors
    fn save_service(&self, id: &str, service: &VersionedServiceTree) -> error::Result<()>;

    /// Persists `credentials` under `id`.
    ///
    /// # Errors
    fn save_credentials(&self, id: &str, credentials: &Authentication) -> error::Result<()>;
}
