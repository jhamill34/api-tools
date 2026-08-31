//! The `service_loader` output/input ports ([`LoaderOutput`], [`Fetcher`]),
//! moved out of `service_loader` so a crate that only implements one of
//! these doesn't have to depend on `service_loader`'s own OpenAPI-parsing
//! logic.

use std::io;

use thiserror::Error;

use crate::service::VersionedServiceTree;
use credential_entities::credentials::Authentication;

/// Failure modes of a [`LoaderOutput`] implementation - deliberately a
/// small, port-contract-shaped enum, not `service_loader`'s own (much
/// larger) internal parsing-error type: a storage sink only ever produces
/// "not found"/"I/O failed"/"some other adapter-specific error", never a
/// `$ref`-cycle or malformed-YAML failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LoaderOutputError {
    /// A required piece of the input was missing.
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
        /// The wrapped error from the [`LoaderOutput`] implementation.
        source: anyhow::Error,
    },
}

/// Shorthand for a [`Result`](core::result::Result) using
/// [`LoaderOutputError`] as its error type.
pub type Result<T> = core::result::Result<T, LoaderOutputError>;

/// An output port a service loader writes loaded data to.
pub trait LoaderOutput {
    /// Stores a loaded service manifest under `id`.
    ///
    /// # Errors
    fn handle_service(&mut self, id: &str, service: VersionedServiceTree) -> Result<()>;

    /// Stores loaded credentials under `id`.
    ///
    /// # Errors
    fn handle_credentials(&mut self, id: &str, credentials: Authentication) -> Result<()>;
}

/// An input port a service loader reads from: opens a readable source for
/// a given `location`.
pub trait Fetcher<R>
where
    R: io::Read,
{
    /// Opens `location` for reading.
    ///
    /// # Errors
    fn fetch(&self, location: &str) -> io::Result<R>;
}
