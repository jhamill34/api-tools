#![warn(clippy::restriction, clippy::pedantic)]
#![allow(
    clippy::blanket_clippy_restriction_lints,
    clippy::mod_module_files,
    clippy::self_named_module_files,

    clippy::implicit_return,
    clippy::shadow_reuse,
    clippy::match_ref_pats,

    // Would like to turn on (Configured to 50?)
    clippy::too_many_lines,
    clippy::absolute_paths
)]

//! A local-filesystem adapter implementing [`service_loader`]'s [`Fetcher`]
//! and [`service_writer`]'s [`Storage`] output ports.

use std::{fs::File, path::PathBuf};

use service_loader::Fetcher;
use service_writer::Storage;

/// Reads and writes files on the local filesystem, resolving every
/// `location` relative to a fixed working directory.
#[derive(Clone)]
pub struct LocalFileFetcher {
    /// The working directory every `location` is resolved relative to.
    cwd: PathBuf,
}

impl From<PathBuf> for LocalFileFetcher {
    /// Uses `value` as the working directory locations are resolved against.
    #[inline]
    fn from(value: PathBuf) -> Self {
        Self { cwd: value }
    }
}

impl Fetcher<File> for LocalFileFetcher {
    /// Opens `location` (joined onto [`cwd`](LocalFileFetcher::from)) for
    /// reading.
    #[inline]
    fn fetch(&self, location: &str) -> std::io::Result<File> {
        let next_file = self.cwd.join(location);
        File::open(next_file)
    }
}

impl Storage<File> for LocalFileFetcher {
    /// Creates (or truncates) `location` (joined onto
    /// [`cwd`](LocalFileFetcher::from)) for writing.
    #[inline]
    fn store(&self, location: &str) -> std::io::Result<File> {
        let file = self.cwd.join(location);
        File::create(file)
    }
}
