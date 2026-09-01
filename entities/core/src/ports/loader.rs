//! The `service_loader` input port ([`Fetcher`]), moved out of
//! `service_loader` so a crate that only implements it doesn't have to
//! depend on `service_loader`'s own OpenAPI-parsing logic.
//!
//! `service_loader` has no output port of its own: its `ServiceLoader`'s
//! `load_service`/`load_credentials` just return the parsed data, leaving
//! it to the caller to decide where (if anywhere) it gets stored - see
//! [`super::catalog::ServiceCatalogWriter`] for the port a caller that
//! *does* want to persist a loaded service typically writes through.

use std::io;

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
