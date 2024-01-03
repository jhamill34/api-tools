#![warn(clippy::restriction, clippy::pedantic)]
#![allow(
    clippy::blanket_clippy_restriction_lints,
    clippy::mod_module_files,
    clippy::self_named_module_files,

    clippy::implicit_return,
    clippy::shadow_reuse,
    clippy::match_ref_pats,

    // Would like to turn on (Configured to 50?)
    clippy::too_many_lines
)]

//!

use std::{fs::File, path::PathBuf};

use service_loader::Fetcher;
use service_writer::Storage;

///
#[derive(Clone)]
pub struct LocalFileFetcher {
    ///
    cwd: PathBuf,
}

impl From<PathBuf> for LocalFileFetcher {
    ///
    #[inline]
    fn from(value: PathBuf) -> Self {
        Self { cwd: value }
    }
}

impl Fetcher<File> for LocalFileFetcher {
    ///
    #[inline]
    fn fetch(&self, location: &str) -> std::io::Result<File> {
        let next_file = self.cwd.join(location);
        File::open(next_file)
    }
}

impl Storage<File> for LocalFileFetcher {
    ///
    #[inline]
    fn store(&self, location: &str) -> std::io::Result<File> {
        let file = self.cwd.join(location);
        File::create(file)
    }
}
