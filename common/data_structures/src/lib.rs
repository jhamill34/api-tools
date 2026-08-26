//! Small, dependency-free data structures shared across the workspace.
//!
//! A byte-wise, wildcard-aware [`trie`](trie::Trie), and a background-thread
//! [`log_writer`](log_writer::LogWriter) for logging off the hot request
//! path.

pub mod log_writer;
pub mod trie;
