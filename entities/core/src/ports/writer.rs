//! The `service_writer` output port ([`Storage`]) - moved out of
//! `service_writer` so a crate that only implements it doesn't have to
//! depend on `service_writer`'s own OpenAPI-reconstruction logic.

use std::io;

/// An output port a service writer writes to: opens a writable destination
/// for a given `location`.
pub trait Storage<W>
where
    W: io::Write,
{
    /// Opens `location` for writing.
    ///
    /// # Errors
    fn store(&self, location: &str) -> io::Result<W>;
}
