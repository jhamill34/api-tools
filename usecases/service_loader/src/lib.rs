#![warn(clippy::restriction, clippy::pedantic)]
#![allow(
    clippy::blanket_clippy_restriction_lints,
    clippy::mod_module_files,
    clippy::self_named_module_files,
    clippy::implicit_return,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::too_many_lines,
    clippy::question_mark_used,
    clippy::needless_borrowed_reference,
    clippy::absolute_paths,
    clippy::ref_patterns,
    clippy::single_call_fn
)]

//! Loads a service manifest (currently OpenAPI-based), its credentials, and
//! its override configuration from a [`Fetcher`] source into a
//! [`LoaderOutput`] sink.

mod constants;
mod loaders;

pub mod error;

use std::io;

use core_entities::service::{SwaggerOverrides, VersionedServiceTree};
use credential_entities::credentials::Authentication;
use loaders::{load_configuration, load_credentials, load_service};

/// An output port [`ServiceLoader`] writes loaded data to.
pub trait LoaderOutput {
    /// Stores a loaded service manifest under `id`.
    ///
    /// # Errors
    fn handle_service(&mut self, id: &str, service: VersionedServiceTree) -> error::Result<()>;

    /// Stores loaded credentials under `id`.
    ///
    /// # Errors
    fn handle_credentials(&mut self, id: &str, credentials: Authentication) -> error::Result<()>;
}

/// An input port [`ServiceLoader`] reads from: opens a readable source for
/// a given `location`.
pub trait Fetcher<R>
where
    R: io::Read,
{
    /// Opens `location` for reading.
    ///
    /// # Errors
    fn fetch(&self, location: &str) -> io::Result<R>;
}

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
    let service = service.mut_v1();

    let api = service
        .commonApi
        .as_mut()
        .ok_or_else(|| error::ServiceLoader::NotFound("Common API".into()))?;

    let mut base_path = api.basePath().to_owned();
    if !overrides.baseUrl.is_empty() {
        if base_path.contains("{{baseUrl}}") {
            base_path = base_path.replace("{{baseUrl}}", &overrides.baseUrl);
        } else {
            base_path = overrides.baseUrl.clone();
        }
    }

    // Set server variables
    for (key, value) in &overrides.serverVariables {
        let key = ["{", key, "}"].join("");
        base_path = base_path.replace(&key, value);
    }

    api.set_basePath(base_path);

    let manifest = service
        .manifest
        .as_mut()
        .ok_or_else(|| error::ServiceLoader::NotFound("Service Manifest".into()))?;
    let manifest = manifest.mut_v2().mut_swagger();

    if manifest.auth.has_oauthConfig() {
        let oauth_config = manifest
            .auth
            .as_mut()
            .ok_or_else(|| error::ServiceLoader::NotFound("Auth Configuration".into()))?;
        let oauth_config = oauth_config.mut_oauthConfig();

        if let &Some(core_entities::service::swagger_overrides::AuthOverrides::OauthConfig(
            ref oauth_config_override,
        )) = &overrides.authOverrides
        {
            apply_if_exists!(name, oauth_config_override => oauth_config);
            apply_if_exists!(authUri, oauth_config_override => oauth_config);
            apply_if_exists!(accessTokenUri, oauth_config_override => oauth_config);
            apply_if_exists!(responseType, oauth_config_override => oauth_config);
            apply_if_exists!(prompt, oauth_config_override => oauth_config);
            apply_if_exists!(oauthDocumentation, oauth_config_override => oauth_config);
            apply_if_exists!(accessTokenMethod, oauth_config_override => oauth_config);
            apply_if_exists!(scope, oauth_config_override => oauth_config);
            // apply_if_exists!(parameterLocation, oauth_config_override => oauth_config);
            // apply_if_exists!(needsBasicAuthHeader, oauth_config_override => oauth_config);
            apply_if_exists!(accessTokenPath, oauth_config_override => oauth_config);
            apply_if_exists!(enableGroupCredentials, oauth_config_override => oauth_config);
            apply_if_exists!(audience, oauth_config_override => oauth_config);
            // apply_if_exists!(grantType, oauth_config_override => oauth_config);
        }

        if oauth_config.authUri.contains("{{baseUrl}}") {
            oauth_config.authUri = oauth_config
                .authUri
                .replace("{{baseUrl}}", &overrides.baseUrl);
        }

        if oauth_config.accessTokenUri.contains("{{baseUrl}}") {
            oauth_config.accessTokenUri = oauth_config
                .accessTokenUri
                .replace("{{baseUrl}}", &overrides.baseUrl);
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

        if !only_manifest && value.v1().manifest.v2().has_swagger() {
            let creds = load_credentials(fetcher);
            if let Ok(creds) = creds {
                output.handle_credentials(id, creds)?;
            }

            if merge_overrides {
                let config = load_configuration(fetcher);
                if let Ok(config) = config {
                    merge(&mut value, &config)?;
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
