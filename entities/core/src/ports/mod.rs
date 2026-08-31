//! Ports (traits) that other crates implement to drive or extend the
//! execution engine, the service loader, and the service writer - moved
//! here so a crate that only wants to *call* one of these interfaces
//! doesn't have to depend on whichever crate happens to implement it.
//!
//! Not every port lives here: a driving port (e.g. `service_loader`'s
//! `ServiceLoaderPort`) whose only implementer is its own crate's concrete
//! type doesn't reduce anyone's dependency footprint by moving, so those
//! stay put.

pub mod engine;
pub mod loader;
pub mod writer;
