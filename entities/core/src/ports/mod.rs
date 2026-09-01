//! Ports (traits) that other crates implement to drive or extend the
//! execution engine and the service catalog - moved here so a crate that
//! only wants to *call* one of these interfaces doesn't have to depend on
//! whichever crate happens to implement it.
//!
//! Not every driving surface gets a trait here: `service_loader` and
//! `service_writer` each have exactly one implementation (their own
//! `ServiceLoader`/`ServiceWriter`), co-located with the trait that would
//! wrap it - a caller always depends on the whole crate either way, so a
//! port there wouldn't reduce anyone's dependency footprint. They're called
//! directly as concrete types instead.

pub mod catalog;
pub mod engine;
pub mod loader;
pub mod value;
pub mod writer;
