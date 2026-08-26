//! Loads a service's manifest, credentials, and override configuration
//! from their well-known [`constants`](crate::constants) locations.

mod openapi;

use std::{collections::HashMap, io};

use crate::Fetcher;

use super::{constants, error};
use core_entities::service::{
    ServiceManifest, ServiceResource, SwaggerOverrides, VersionedServiceTree,
};
use credential_entities::credentials::Authentication;

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

    let result = protobuf_json_mapping::parse_from_str(&config)?;
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
    let creds: Authentication = protobuf_json_mapping::parse_from_str(&creds)?;

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
    let manifest: ServiceManifest = protobuf_json_mapping::parse_from_str(&manifest)?;

    let mut tree = VersionedServiceTree::new();

    let v1 = tree.mut_v1();
    v1.manifest = protobuf::MessageField::some(manifest);

    if !only_manifest {
        let latest_manifest = v1.manifest.v2();

        if latest_manifest.has_action() {
            let action = latest_manifest.action();
            let root = &action.source;

            for operation in &action.operations {
                if operation.has_function() {
                    let func = operation.function();
                    let path = &[root, func.js()].join("/");

                    let source = fetcher.fetch(path)?;
                    let source = io::read_to_string(source)?;

                    let mut resource = ServiceResource::new();
                    resource.relativePath = path.to_string();
                    resource.content = source;

                    v1.resources.push(resource);
                }
            }
        }

        if latest_manifest.has_swagger() {
            let swagger = latest_manifest.swagger();
            v1.commonApi = openapi::handle(fetcher, &swagger.source)?;
        }
    }

    Ok(tree)
}
