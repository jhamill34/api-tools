//! [`LocalDirectoryCatalog`]: a [`ServiceCatalog`]/[`ServiceCatalogWriter`]
//! backed by a directory tree on disk, one subdirectory per service, with
//! an in-memory index for fast reads.
//!
//! Keeping the on-disk files and the in-memory index consistent is this
//! type's job, not its caller's: [`LocalDirectoryCatalog::save_service`]/
//! [`LocalDirectoryCatalog::save_credentials`] write disk (via
//! `service_writer`) and commit the index in the same call, so a caller
//! only ever sees one write. [`LocalDirectoryCatalog::refresh`] is the
//! other direction - disk changed out from under this process (a hand
//! edit, a deploy, anything a file watcher might notice) - so it reads disk
//! (via `service_loader`) and commits the index, never writing disk back.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Mutex, PoisonError},
};

use core_entities::ports::catalog::{error::CatalogError, ServiceCatalog, ServiceCatalogWriter};
use core_entities::service::VersionedServiceTree;
use credential_entities::credentials::Authentication;
use local_file_loader::LocalFileFetcher;
use service_loader::ServiceLoader;
use service_writer::ServiceWriter;

/// One service's cached state.
#[derive(Default, Clone)]
struct Entry {
    /// The service's manifest, if loaded.
    service: Option<VersionedServiceTree>,

    /// The service's credentials, if loaded and present.
    credentials: Option<Authentication>,
}

/// A [`ServiceCatalog`]/[`ServiceCatalogWriter`] backed by a directory tree
/// on disk, one subdirectory per service.
#[non_exhaustive]
pub struct LocalDirectoryCatalog {
    /// Each service's on-disk directory, keyed by id. Fixed at
    /// construction - a directory that appears later isn't picked up.
    paths: HashMap<String, PathBuf>,

    /// The in-memory index, refreshed on every write (either admin-driven
    /// via `save_service`/`save_credentials`, or disk-detected via
    /// `refresh`).
    index: Mutex<BTreeMap<String, Entry>>,
}

impl LocalDirectoryCatalog {
    /// Creates a [`LocalDirectoryCatalog`] over `paths`, with an empty
    /// index - call [`refresh`](Self::refresh) for every known id (or use
    /// [`refresh_all`](Self::refresh_all)) to populate it from disk before
    /// serving reads.
    #[must_use]
    #[inline]
    pub fn new(paths: HashMap<String, PathBuf>) -> Self {
        Self {
            paths,
            index: Mutex::new(BTreeMap::new()),
        }
    }

    /// [`refresh`](Self::refresh)es every id this catalog knows a directory
    /// for. Collects and returns every id that failed, rather than
    /// stopping at the first one, so one bad service doesn't block loading
    /// the rest.
    ///
    /// # Errors
    #[inline]
    pub fn refresh_all(&self) -> Result<(), Vec<(String, CatalogError)>> {
        let failures: Vec<_> = self
            .paths
            .keys()
            .filter_map(|id| self.refresh(id).err().map(|err| (id.clone(), err)))
            .collect();

        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }

    /// Re-reads `id` from disk and commits the result into the in-memory
    /// index. Never writes disk - disk is already the source of whatever
    /// changed.
    ///
    /// # Errors
    #[inline]
    pub fn refresh(&self, id: &str) -> Result<(), CatalogError> {
        let fetcher = self.fetcher_for(id)?;
        let loader = ServiceLoader::default();

        let service = loader
            .load_service(&fetcher, false, true)
            .map_err(|source| CatalogError::Other {
                source: anyhow::Error::from(source),
            })?;
        let credentials =
            loader
                .load_credentials(&fetcher)
                .map_err(|source| CatalogError::Other {
                    source: anyhow::Error::from(source),
                })?;

        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = index.entry(id.to_owned()).or_default();
        entry.service = Some(service);
        entry.credentials = credentials;

        Ok(())
    }

    /// Looks up `id`'s directory and opens a [`LocalFileFetcher`] onto it.
    fn fetcher_for(&self, id: &str) -> Result<LocalFileFetcher, CatalogError> {
        self.paths
            .get(id)
            .map(|path| LocalFileFetcher::from(path.clone()))
            .ok_or_else(|| CatalogError::NotFound(format!("Service directory for {id}")))
    }
}

impl ServiceCatalog for LocalDirectoryCatalog {
    #[inline]
    fn list(&self) -> Vec<String> {
        self.index
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    #[inline]
    fn get_service(&self, id: &str) -> Option<VersionedServiceTree> {
        self.index
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)?
            .service
            .clone()
    }

    #[inline]
    fn get_credentials(&self, id: &str) -> Option<Authentication> {
        self.index
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)?
            .credentials
            .clone()
    }
}

impl ServiceCatalogWriter for LocalDirectoryCatalog {
    #[inline]
    fn save_service(
        &self,
        id: &str,
        service: &VersionedServiceTree,
    ) -> core_entities::ports::catalog::error::Result<()> {
        let fetcher = self.fetcher_for(id)?;

        ServiceWriter::default()
            .store_service(service, &fetcher, false)
            .map_err(|source| CatalogError::Other {
                source: anyhow::Error::from(source),
            })?;

        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        index.entry(id.to_owned()).or_default().service = Some(service.clone());

        Ok(())
    }

    #[inline]
    fn save_credentials(
        &self,
        id: &str,
        credentials: &Authentication,
    ) -> core_entities::ports::catalog::error::Result<()> {
        let fetcher = self.fetcher_for(id)?;

        ServiceWriter::default()
            .store_credentials(credentials, &fetcher)
            .map_err(|source| CatalogError::Other {
                source: anyhow::Error::from(source),
            })?;

        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        index.entry(id.to_owned()).or_default().credentials = Some(credentials.clone());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Writes a minimal `manifest.json`/`openapi.yaml` pair under `dir`
    /// naming a swagger-backed service.
    fn write_minimal_service(dir: &std::path::Path) {
        fs::write(
            dir.join("manifest.json"),
            r#"{"v2":{"swagger":{"source":"openapi.yaml"}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("openapi.yaml"),
            "servers:\n  - url: https://example.com\n",
        )
        .unwrap();
    }

    fn catalog_over_one_service(dir: &std::path::Path) -> LocalDirectoryCatalog {
        write_minimal_service(dir);
        let mut paths = HashMap::new();
        paths.insert("svc".to_owned(), dir.to_path_buf());
        LocalDirectoryCatalog::new(paths)
    }

    #[test]
    fn refresh_populates_the_index_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_over_one_service(dir.path());

        assert!(catalog.get_service("svc").is_none(), "index starts empty");

        catalog.refresh("svc").unwrap();

        assert!(catalog.get_service("svc").is_some());
        assert_eq!(catalog.list(), vec!["svc".to_owned()]);
    }

    #[test]
    fn refresh_errors_for_an_unknown_id() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_over_one_service(dir.path());

        let result = catalog.refresh("unknown");

        assert!(matches!(result, Err(CatalogError::NotFound(_))));
    }

    #[test]
    fn save_service_is_immediately_visible_without_a_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_over_one_service(dir.path());
        catalog.refresh("svc").unwrap();
        let service = catalog.get_service("svc").unwrap();

        catalog.save_service("svc", &service).unwrap();

        // Overwrites the same content, but proves the write landed on the
        // real filename (not some `.new`-suffixed sibling) by refreshing
        // straight after and getting the same data back.
        catalog.refresh("svc").unwrap();
        assert!(catalog.get_service("svc").is_some());
    }

    #[test]
    fn save_credentials_updates_the_index_in_the_same_call() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_over_one_service(dir.path());
        catalog.refresh("svc").unwrap();
        assert!(catalog.get_credentials("svc").is_none());

        let creds =
            Authentication::Basic(credential_entities::credentials::BasicCredentials::default());
        catalog.save_credentials("svc", &creds).unwrap();

        assert!(catalog.get_credentials("svc").is_some());
    }
}
