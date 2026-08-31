//! Loads a service manifest (currently OpenAPI-based), its credentials, and
//! its override configuration from a [`Fetcher`] source into a
//! [`LoaderOutput`] sink.

mod constants;
mod loaders;

pub mod error;

use std::io;

use core_entities::entity::{
    service_manifest, service_manifest_latest, versioned_service_tree, ServiceManifestLatest,
    SwaggerOverrides, VersionedServiceTree,
};
pub use core_entities::ports::loader::{Fetcher, LoaderOutput};
use loaders::{load_configuration, load_credentials, load_service};

/// Copies `$field` from `$source` onto `$sink` only if it's non-empty on
/// `$source`, leaving `$sink`'s existing value untouched otherwise.
macro_rules! apply_if_exists {
    ($field:ident, $source:expr => $sink:expr) => {
        if !$source.$field.is_empty() {
            $sink.$field = $source.$field.clone();
        }
    };
}

/// Applies `overrides` onto a loaded `service`'s base path and, if present,
/// OAuth configuration — substituting `{{baseUrl}}`/server-variable
/// placeholders and copying any non-empty override fields onto the
/// matching OAuth config fields via `apply_if_exists!`.
///
/// # Errors
/// # Panics
#[inline]
pub fn merge(
    service: &mut VersionedServiceTree,
    overrides: &SwaggerOverrides,
) -> error::Result<()> {
    if !matches!(&service.version, Some(versioned_service_tree::Version::V1(_))) {
        service.version = Some(versioned_service_tree::Version::V1(
            versioned_service_tree::V1::default(),
        ));
    }
    let Some(versioned_service_tree::Version::V1(service)) = &mut service.version else {
        unreachable!("just set to Some(Version::V1(_)) above")
    };

    let api = service
        .common_api
        .as_mut()
        .ok_or_else(|| error::ServiceLoader::NotFound("Common API".into()))?;

    let mut base_path = api.base_path.clone().unwrap_or_default();
    if !overrides.base_url.is_empty() {
        if base_path.contains("{{baseUrl}}") {
            base_path = base_path.replace("{{baseUrl}}", &overrides.base_url);
        } else {
            base_path.clone_from(&overrides.base_url);
        }
    }

    // Set server variables
    for (key, value) in &overrides.server_variables {
        let key = ["{", key, "}"].join("");
        base_path = base_path.replace(&key, value);
    }

    api.base_path = Some(base_path);

    let manifest = service
        .manifest
        .as_mut()
        .ok_or_else(|| error::ServiceLoader::NotFound("Service Manifest".into()))?;
    if !matches!(&manifest.value, Some(service_manifest::Value::V2(_))) {
        manifest.value = Some(service_manifest::Value::V2(ServiceManifestLatest::default()));
    }
    let Some(service_manifest::Value::V2(latest)) = &mut manifest.value else {
        unreachable!("just set to Some(Value::V2(_)) above")
    };
    if !matches!(
        &latest.value,
        Some(service_manifest_latest::Value::Swagger(_))
    ) {
        latest.value = Some(service_manifest_latest::Value::Swagger(Box::default()));
    }
    let Some(service_manifest_latest::Value::Swagger(manifest)) = &mut latest.value else {
        unreachable!("just set to Some(Value::Swagger(_)) above")
    };

    if let Some(oauth_config) = manifest.auth.as_mut().and_then(|auth| auth.oauth_config.as_mut()) {
        if let Some(oauth_config_override) = &overrides.oauth_config {
            apply_if_exists!(name, oauth_config_override => oauth_config);
            apply_if_exists!(auth_uri, oauth_config_override => oauth_config);
            apply_if_exists!(access_token_uri, oauth_config_override => oauth_config);
            apply_if_exists!(response_type, oauth_config_override => oauth_config);
            apply_if_exists!(prompt, oauth_config_override => oauth_config);
            apply_if_exists!(oauth_documentation, oauth_config_override => oauth_config);
            apply_if_exists!(access_token_method, oauth_config_override => oauth_config);
            apply_if_exists!(scope, oauth_config_override => oauth_config);
            // apply_if_exists!(parameter_location, oauth_config_override => oauth_config);
            // apply_if_exists!(needs_basic_auth_header, oauth_config_override => oauth_config);
            apply_if_exists!(access_token_path, oauth_config_override => oauth_config);
            apply_if_exists!(enable_group_credentials, oauth_config_override => oauth_config);
            apply_if_exists!(audience, oauth_config_override => oauth_config);
            // apply_if_exists!(grant_type, oauth_config_override => oauth_config);
        }

        if oauth_config.auth_uri.contains("{{baseUrl}}") {
            oauth_config.auth_uri = oauth_config
                .auth_uri
                .replace("{{baseUrl}}", &overrides.base_url);
        }

        if oauth_config.access_token_uri.contains("{{baseUrl}}") {
            oauth_config.access_token_uri = oauth_config
                .access_token_uri
                .replace("{{baseUrl}}", &overrides.base_url);
        }
    }

    Ok(())
}

/// Loads a service manifest, and optionally its credentials and override
/// configuration, from a [`Fetcher`] into a [`LoaderOutput`].
#[non_exhaustive]
pub struct ServiceLoader;

impl ServiceLoader {
    /// Creates a [`ServiceLoader`].
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self
    }

    /// Loads the service manifest at `fetcher`'s well-known locations and
    /// stores it in `output` under `id`. Unless `only_manifest` is set,
    /// also loads credentials (if present) and, when `merge_overrides` is
    /// set, loads and applies override configuration via [`merge`] before
    /// storing.
    ///
    /// # Errors
    #[inline]
    pub fn load<R: io::Read>(
        &self,
        id: &str,
        fetcher: &dyn Fetcher<R>,
        output: &mut dyn LoaderOutput,
        merge_overrides: bool,
        only_manifest: bool,
    ) -> error::Result<()> {
        let mut value = load_service(fetcher, only_manifest)?;

        if !only_manifest
            && matches!(
                &value.v1().manifest_latest().value,
                Some(service_manifest_latest::Value::Swagger(_))
            )
        {
            match load_credentials(fetcher) {
                Ok(creds) => output.handle_credentials(id, creds)?,
                Err(error::ServiceLoader::Io { source })
                    if source.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }

            if merge_overrides {
                match load_configuration(fetcher) {
                    Ok(config) => merge(&mut value, &config)?,
                    Err(error::ServiceLoader::Io { source })
                        if source.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err),
                }
            }
        }

        output.handle_service(id, value)?;

        Ok(())
    }
}

impl Default for ServiceLoader {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// A primary/driving port: the behavioral surface a driving adapter (e.g.
/// `apid`'s background loader) calls to load a service. Unlike [`Fetcher`]/
/// [`LoaderOutput`] - which [`ServiceLoader`] itself calls *out* through -
/// this one is implemented *by* [`ServiceLoader`] and called *into* by
/// whoever is driving it, so a caller can depend on this interface instead
/// of the concrete [`ServiceLoader`] type.
pub trait ServiceLoaderPort<R>
where
    R: io::Read,
{
    /// See [`ServiceLoader::load`].
    ///
    /// # Errors
    fn load(
        &self,
        id: &str,
        fetcher: &dyn Fetcher<R>,
        output: &mut dyn LoaderOutput,
        merge_overrides: bool,
        only_manifest: bool,
    ) -> error::Result<()>;
}

impl<R> ServiceLoaderPort<R> for ServiceLoader
where
    R: io::Read,
{
    #[inline]
    fn load(
        &self,
        id: &str,
        fetcher: &dyn Fetcher<R>,
        output: &mut dyn LoaderOutput,
        merge_overrides: bool,
        only_manifest: bool,
    ) -> error::Result<()> {
        self.load(id, fetcher, output, merge_overrides, only_manifest)
    }
}

#[cfg(test)]
mod test {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use credential_entities::entity::Authentication;

    use super::*;

    #[derive(Default)]
    struct MockFetcher {
        docs: RefCell<HashMap<String, String>>,
    }

    impl MockFetcher {
        fn with(self, location: &str, content: &str) -> Self {
            self.docs
                .borrow_mut()
                .insert(location.to_owned(), content.to_owned());
            self
        }
    }

    impl Fetcher<io::Cursor<Vec<u8>>> for MockFetcher {
        fn fetch(&self, location: &str) -> io::Result<io::Cursor<Vec<u8>>> {
            self.docs
                .borrow()
                .get(location)
                .map(|doc| io::Cursor::new(doc.clone().into_bytes()))
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "not found"))
        }
    }

    #[derive(Default)]
    struct MockOutput {
        credentials: Option<Authentication>,
    }

    impl LoaderOutput for MockOutput {
        fn handle_service(
            &mut self,
            _id: &str,
            _service: VersionedServiceTree,
        ) -> core_entities::ports::loader::Result<()> {
            Ok(())
        }

        fn handle_credentials(
            &mut self,
            _id: &str,
            credentials: Authentication,
        ) -> core_entities::ports::loader::Result<()> {
            self.credentials = Some(credentials);
            Ok(())
        }
    }

    fn manifest_with_swagger() -> String {
        r#"{"v2":{"swagger":{"source":"openapi"}}}"#.to_owned()
    }

    #[test]
    fn load_skips_missing_credentials_and_config_files() {
        let openapi_doc = include_str!("loaders/openapi/stubs/basic_root.yaml");
        let fetcher = MockFetcher::default()
            .with(constants::MANIFEST_LOCATION, &manifest_with_swagger())
            .with("openapi", openapi_doc);
        let mut output = MockOutput::default();

        let result = ServiceLoader::new().load("svc", &fetcher, &mut output, true, false);

        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert!(output.credentials.is_none());
    }

    #[test]
    fn load_propagates_malformed_credentials_instead_of_swallowing_them() {
        let openapi_doc = include_str!("loaders/openapi/stubs/basic_root.yaml");
        let fetcher = MockFetcher::default()
            .with(constants::MANIFEST_LOCATION, &manifest_with_swagger())
            .with("openapi", openapi_doc)
            .with(constants::CREDENTIALS_LOCATION, "not valid json{{{");
        let mut output = MockOutput::default();

        let result = ServiceLoader::new().load("svc", &fetcher, &mut output, false, false);

        assert!(
            result.is_err(),
            "expected malformed credentials to surface as an error, got Ok"
        );
        assert!(output.credentials.is_none());
    }

    #[test]
    fn load_propagates_malformed_config_instead_of_swallowing_it() {
        let openapi_doc = include_str!("loaders/openapi/stubs/basic_root.yaml");
        let fetcher = MockFetcher::default()
            .with(constants::MANIFEST_LOCATION, &manifest_with_swagger())
            .with("openapi", openapi_doc)
            .with(constants::CONFIG_LOCATION, "not valid json{{{");
        let mut output = MockOutput::default();

        let result = ServiceLoader::new().load("svc", &fetcher, &mut output, true, false);

        assert!(
            result.is_err(),
            "expected malformed config to surface as an error, got Ok"
        );
    }
}
