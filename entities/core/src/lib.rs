//! Hand-written domain entities for a loaded service manifest.
//!
//! `entity` mirrors the subset of `src/proto/service.proto` that's
//! actually referenced anywhere in this workspace - see that module's own
//! doc comment for exactly what was intentionally left out. It's
//! temporarily mounted here as `entity` rather than `service` while
//! consumers migrate off the `protobuf`-generated `service` module below,
//! one crate at a time; once every consumer has moved onto `entity`,
//! `service` (and this crate's `protobuf`/`protobuf-codegen` dependency)
//! gets deleted and `entity` takes over the `service` name.

include!(concat!(env!("OUT_DIR"), "/proto/mod.rs"));

pub mod entity;
