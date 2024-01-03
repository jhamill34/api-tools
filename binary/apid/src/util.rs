//!

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::anyhow;

///
pub fn is_hidden(entry: &Path) -> bool {
    entry
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map_or(true, |name| name.starts_with('.'))
}

///
pub fn get_paths(path: &Path) -> anyhow::Result<impl Iterator<Item = PathBuf>> {
    if path.is_dir() {
        let iter = fs::read_dir(path)?
            .filter_map(core::result::Result::ok)
            .map(|dir| dir.path())
            .filter(|dir| dir.is_dir() && !is_hidden(dir));

        Ok(iter)
    } else {
        Err(anyhow!("Not a directory"))
    }
}
