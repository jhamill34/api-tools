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
    clippy::needless_borrowed_reference
)]

//!

mod constants;
mod loaders;

pub mod error;

use std::io;

use credential_entities::credentials::Authentication;
use loaders::{load_configuration, load_credentials, load_service};
use core_entities::service::{SwaggerOverrides, VersionedServiceTree};

///
pub trait LoaderOutput {
    ///
    /// # Errors
    fn handle_service(&mut self, id: &str, service: VersionedServiceTree) -> error::Result<()>;

    ///
    /// # Errors
    fn handle_credentials(&mut self, id: &str, credentials: Authentication) -> error::Result<()>;
}

///
pub trait Fetcher<R>
where
    R: io::Read,
{
    ///
    /// # Errors
    fn fetch(&self, location: &str) -> io::Result<R>;
}

///
macro_rules! apply_if_exists {
    ($field:ident, $source:expr => $sink:expr) => {
        if !$source.$field.is_empty() {
            $sink.$field = $source.$field.clone();
        }
    };
}

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

        if let &Some(
            core_entities::service::swagger_overrides::AuthOverrides::OauthConfig(
                ref oauth_config_override,
            ),
        ) = &overrides.authOverrides
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

///
#[non_exhaustive]
pub struct ServiceLoader;

impl ServiceLoader {
    ///
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self
    }

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
