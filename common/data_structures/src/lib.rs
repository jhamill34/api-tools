#![warn(clippy::restriction, clippy::pedantic)]
#![allow(
    clippy::blanket_clippy_restriction_lints,
    clippy::mod_module_files,
    clippy::self_named_module_files,

    clippy::implicit_return,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::match_ref_pats,

    // Would like to turn on (Configured to 50?)
    clippy::too_many_lines
)]

//! Small, dependency-free data structures shared across the workspace.
//!
//! A byte-wise, wildcard-aware [`trie`](trie::Trie), and a background-thread
//! [`log_writer`](log_writer::LogWriter) for logging off the hot request
//! path.

pub mod log_writer;
pub mod trie;
