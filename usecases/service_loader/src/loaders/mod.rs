//!

mod openapi;

use std::{collections::HashMap, io};

use crate::Fetcher;

use super::{constants, error};
use credential_entities::credentials::Authentication;
use core_entities::service::{
    ServiceManifest, ServiceResource, SwaggerOverrides, VersionedServiceTree,
};

///
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

///
fn traverse_map(current: &mut serde_json::Value, parts: &[&str], value: &str) -> error::Result<()> {
    if let Some(next) = parts.first() {
        if let &mut serde_json::Value::Object(ref mut current) = current {
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

///
pub fn load_credentials<R: io::Read>(fetcher: &dyn Fetcher<R>) -> error::Result<Authentication> {
    let creds = fetcher.fetch(constants::CREDENTIALS_LOCATION)?;
    let creds = io::read_to_string(creds)?;
    let creds: Authentication = protobuf_json_mapping::parse_from_str(&creds)?;

    Ok(creds)
}

///
pub fn load_service<R: io::Read>(
    fetcher: &dyn Fetcher<R>,
    only_manifest: bool,
) -> error::Result<VersionedServiceTree> {
    let manifest = fetcher.fetch(constants::MANIFEST_LOCATION)?;
    let manifest = io::read_to_string(manifest)?;
    let manifest: ServiceManifest = protobuf_json_mapping::parse_from_str(&manifest)?;

    let mut tree = VersionedServiceTree::new();

    let mut v1 = tree.mut_v1();
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
