//! Parses a service manifest (currently OpenAPI-based), its credentials,
//! and its override configuration from a [`Fetcher`] source, returning the
//! parsed data - it's up to the caller to decide where (if anywhere) that
//! gets stored.

mod constants;
mod loaders;

pub mod error;

use std::io;

pub use core_entities::ports::loader::Fetcher;
use core_entities::service::{
    service_manifest, service_manifest_latest, versioned_service_tree, ServiceManifestLatest,
    SwaggerOverrides, VersionedServiceTree,
};
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
    if !matches!(
        &service.version,
        Some(versioned_service_tree::Version::V1(_))
    ) {
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

    if let Some(oauth_config) = manifest
        .auth
        .as_mut()
        .and_then(|auth| auth.oauth_config.as_mut())
    {
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

/// Parses a service manifest, and optionally its credentials and override
/// configuration, from a [`Fetcher`].
#[non_exhaustive]
pub struct ServiceLoader;

impl ServiceLoader {
    /// Creates a [`ServiceLoader`].
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self
    }

    /// Parses the service manifest at `fetcher`'s well-known location.
    /// Unless `only_manifest` is set, also resolves its `OpenAPI`
    /// document/action resources and, when `merge_overrides` is set, loads
    /// and applies override configuration via [`merge`].
    ///
    /// # Errors
    #[inline]
    pub fn load_service<R: io::Read>(
        &self,
        fetcher: &dyn Fetcher<R>,
        only_manifest: bool,
        merge_overrides: bool,
    ) -> error::Result<VersionedServiceTree> {
        let mut value = load_service(fetcher, only_manifest)?;

        if !only_manifest
            && merge_overrides
            && matches!(
                &value.v1().manifest_latest().value,
                Some(service_manifest_latest::Value::Swagger(_))
            )
        {
            match load_configuration(fetcher) {
                Ok(config) => merge(&mut value, &config)?,
                Err(error::ServiceLoader::Io { source })
                    if source.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }

        Ok(value)
    }

    /// Parses credentials at `fetcher`'s well-known location, if present.
    ///
    /// # Errors
    #[inline]
    pub fn load_credentials<R: io::Read>(
        &self,
        fetcher: &dyn Fetcher<R>,
    ) -> error::Result<Option<credential_entities::credentials::Authentication>> {
        match load_credentials(fetcher) {
            Ok(creds) => Ok(Some(creds)),
            Err(error::ServiceLoader::Io { source })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }
}

impl Default for ServiceLoader {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// A primary/driving port: the behavioral surface a driving adapter (e.g.
/// `apid`'s background loader) calls to parse a service. Unlike [`Fetcher`] -
/// which [`ServiceLoader`] itself calls *out* through - this one is
/// implemented *by* [`ServiceLoader`] and called *into* by whoever is
/// driving it, so a caller can depend on this interface instead of the
/// concrete [`ServiceLoader`] type.
pub trait ServiceLoaderPort<R>
where
    R: io::Read,
{
    /// See [`ServiceLoader::load_service`].
    ///
    /// # Errors
    fn load_service(
        &self,
        fetcher: &dyn Fetcher<R>,
        only_manifest: bool,
        merge_overrides: bool,
    ) -> error::Result<VersionedServiceTree>;

    /// See [`ServiceLoader::load_credentials`].
    ///
    /// # Errors
    fn load_credentials(
        &self,
        fetcher: &dyn Fetcher<R>,
    ) -> error::Result<Option<credential_entities::credentials::Authentication>>;
}

impl<R> ServiceLoaderPort<R> for ServiceLoader
where
    R: io::Read,
{
    #[inline]
    fn load_service(
        &self,
        fetcher: &dyn Fetcher<R>,
        only_manifest: bool,
        merge_overrides: bool,
    ) -> error::Result<VersionedServiceTree> {
        self.load_service(fetcher, only_manifest, merge_overrides)
    }

    #[inline]
    fn load_credentials(
        &self,
        fetcher: &dyn Fetcher<R>,
    ) -> error::Result<Option<credential_entities::credentials::Authentication>> {
        self.load_credentials(fetcher)
    }
}

#[cfg(test)]
mod test {
    use std::cell::RefCell;
    use std::collections::HashMap;

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

    fn manifest_with_swagger() -> String {
        r#"{"v2":{"swagger":{"source":"openapi"}}}"#.to_owned()
    }

    #[test]
    fn load_credentials_returns_none_when_the_file_is_missing() {
        let fetcher = MockFetcher::default();

        let result = ServiceLoader::new().load_credentials(&fetcher);

        assert!(
            matches!(result, Ok(None)),
            "expected Ok(None), got {result:?}"
        );
    }

    #[test]
    fn load_credentials_propagates_malformed_credentials_instead_of_swallowing_them() {
        let fetcher =
            MockFetcher::default().with(constants::CREDENTIALS_LOCATION, "not valid json{{{");

        let result = ServiceLoader::new().load_credentials(&fetcher);

        assert!(
            result.is_err(),
            "expected malformed credentials to surface as an error, got {result:?}"
        );
    }

    #[test]
    fn load_service_skips_a_missing_config_file_when_merging_overrides() {
        let openapi_doc = include_str!("loaders/openapi/stubs/basic_root.yaml");
        let fetcher = MockFetcher::default()
            .with(constants::MANIFEST_LOCATION, &manifest_with_swagger())
            .with("openapi", openapi_doc);

        let result = ServiceLoader::new().load_service(&fetcher, false, true);

        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[test]
    fn load_service_propagates_malformed_config_instead_of_swallowing_it() {
        let openapi_doc = include_str!("loaders/openapi/stubs/basic_root.yaml");
        let fetcher = MockFetcher::default()
            .with(constants::MANIFEST_LOCATION, &manifest_with_swagger())
            .with("openapi", openapi_doc)
            .with(constants::CONFIG_LOCATION, "not valid json{{{");

        let result = ServiceLoader::new().load_service(&fetcher, false, true);

        assert!(
            result.is_err(),
            "expected malformed config to surface as an error, got Ok"
        );
    }

    #[test]
    fn load_service_does_not_attempt_config_when_merge_overrides_is_unset() {
        let openapi_doc = include_str!("loaders/openapi/stubs/basic_root.yaml");
        let fetcher = MockFetcher::default()
            .with(constants::MANIFEST_LOCATION, &manifest_with_swagger())
            .with("openapi", openapi_doc)
            .with(constants::CONFIG_LOCATION, "not valid json{{{");

        let result = ServiceLoader::new().load_service(&fetcher, false, false);

        assert!(
            result.is_ok(),
            "expected malformed config to be ignored when merge_overrides is unset, got {result:?}"
        );
    }
}
