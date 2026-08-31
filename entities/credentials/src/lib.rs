//! Hand-written credential entities.
//!
//! `entity` mirrors `src/proto/credentials.proto` (every message in it is
//! used elsewhere in the workspace, so nothing was dropped). It's
//! temporarily mounted here as `entity` rather than `credentials` while
//! consumers migrate off the `protobuf`-generated `credentials` module
//! below, one crate at a time; once every consumer has moved onto
//! `entity`, `credentials` (and this crate's `protobuf`/`protobuf-codegen`
//! dependency) gets deleted and `entity` takes over the `credentials`
//! name.

include!(concat!(env!("OUT_DIR"), "/proto/mod.rs"));

pub mod entity;

#[cfg(test)]
mod json_compat_tests;
