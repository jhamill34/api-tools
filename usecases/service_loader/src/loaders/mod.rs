//! Loads a service's manifest, credentials, and override configuration
//! from their well-known [`constants`](crate::constants) locations.

mod openapi;

use std::{collections::HashMap, io};

use crate::Fetcher;

use super::{constants, error};
use core_entities::entity::{
    service_manifest_latest, versioned_service_tree, ServiceManifest, ServiceResource,
    SwaggerOverrides, VersionedServiceTree,
};
use credential_entities::entity::Authentication;

/// Loads override configuration from `fetcher`'s
/// [`CONFIG_LOCATION`](constants::CONFIG_LOCATION) — a flat
/// `key.path.segments` JSON map — expanding each key into a nested
/// structure via [`traverse_map`] before parsing it as [`SwaggerOverrides`].
pub fn load_configuration<R: io::Read>(
    fetcher: &dyn Fetcher<R>,
) -> error::Result<SwaggerOverrides> {
    let config = fetcher.fetch(constants::CONFIG_LOCATION)?;
    let config: HashMap<String, String> = serde_json::from_reader(config)?;

    let mut root = serde_json::Value::Object(serde_json::Map::new());
    for (key, value) in config {
        let parts: Vec<_> = key.split('.').collect();
        traverse_map(&mut root, &parts, &value)?;
    }

    let config = serde_json::to_string(&root)?;

    let result = serde_json::from_str(&config)?;
    Ok(result)
}

/// Writes `value` into `current` at the dot-separated path `parts`,
/// creating intermediate JSON objects along the way. Errors if an
/// intermediate path segment lands on a non-object value.
fn traverse_map(current: &mut serde_json::Value, parts: &[&str], value: &str) -> error::Result<()> {
    if let Some(next) = parts.first() {
        if let serde_json::Value::Object(current) = current {
            let key = (*next).to_owned();
            let child = current
                .entry(key)
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

            let remainder = parts.get(1..).unwrap_or_default();

            traverse_map(child, remainder, value)
        } else {
            Err(error::ServiceLoader::OverrideError(
                "Can only traverse objects".into(),
            ))
        }
    } else {
        *current = serde_json::Value::String(value.to_owned());
        Ok(())
    }
}

/// Loads and parses credentials from `fetcher`'s
/// [`CREDENTIALS_LOCATION`](constants::CREDENTIALS_LOCATION).
pub fn load_credentials<R: io::Read>(fetcher: &dyn Fetcher<R>) -> error::Result<Authentication> {
    let creds = fetcher.fetch(constants::CREDENTIALS_LOCATION)?;
    let creds = io::read_to_string(creds)?;
    let creds: Authentication = serde_json::from_str(&creds)?;

    Ok(creds)
}

/// Loads and parses the manifest at `fetcher`'s
/// [`MANIFEST_LOCATION`](constants::MANIFEST_LOCATION). Unless
/// `only_manifest` is set, also resolves referenced action-script resources
/// and, for an `OpenAPI`-backed manifest, loads and parses its `OpenAPI`
/// document via the `openapi` loader.
pub fn load_service<R: io::Read>(
    fetcher: &dyn Fetcher<R>,
    only_manifest: bool,
) -> error::Result<VersionedServiceTree> {
    let manifest = fetcher.fetch(constants::MANIFEST_LOCATION)?;
    let manifest = io::read_to_string(manifest)?;
    let manifest: ServiceManifest = serde_json::from_str(&manifest)?;

    let mut v1 = versioned_service_tree::V1 {
        manifest: Some(manifest),
        ..Default::default()
    };

    if !only_manifest {
        let latest_manifest = v1.manifest_latest().into_owned();

        if let Some(service_manifest_latest::Value::Action(action)) = &latest_manifest.value {
            let root = &action.source;

            for operation in &action.operations {
                if let Some(func) = &operation.function {
                    let path = &[root, func.js.as_deref().unwrap_or("")].join("/");

                    let source = fetcher.fetch(path)?;
                    let source = io::read_to_string(source)?;

                    let resource = ServiceResource {
                        relative_path: path.clone(),
                        content: source,
                    };

                    v1.resources.push(resource);
                }
            }
        }

        if let Some(service_manifest_latest::Value::Swagger(swagger)) = &latest_manifest.value {
            v1.common_api = Some(openapi::handle(fetcher, &swagger.source)?);
        }
    }

    Ok(VersionedServiceTree {
        version: Some(versioned_service_tree::Version::V1(v1)),
    })
}
