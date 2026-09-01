//! Serializes an internal [`VersionedServiceTree`]/[`Authentication`] pair
//! back out to OpenAPI-shaped JSON/YAML and credential JSON, the inverse of
//! `service_loader`'s `OpenAPI` loader.

use std::{collections::HashMap, io};

pub use core_entities::ports::writer::Storage;
use core_entities::service::{self, VersionedServiceTree};
use credential_entities::credentials::Authentication;

pub mod error;

/// Writes a service manifest or its credentials out through a [`Storage`]
/// adapter.
#[non_exhaustive]
pub struct ServiceWriter;

impl ServiceWriter {
    /// Creates a [`ServiceWriter`].
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Writes `service`'s manifest as pretty-printed JSON to
    /// `./manifest.json`, and — if the manifest has an `OpenAPI`
    /// (`swagger`) source — writes the reconstructed `OpenAPI` document
    /// alongside it via `handle_openapi`. Overwrites whatever was already
    /// there - a save is meant to be immediately visible to the next
    /// reload, not staged for separate review/promotion.
    ///
    /// # Errors
    #[inline]
    pub fn store_service<W: io::Write>(
        &self,
        service: &VersionedServiceTree,
        storage: &dyn Storage<W>,
        split: bool,
    ) -> error::Result<()> {
        let service = service.v1();
        let manifest = service
            .manifest
            .as_ref()
            .ok_or_else(|| error::ServiceWriter::NotFound("Service Manifest".into()))?;

        let manifest_string = serde_json::to_string_pretty(manifest)?;

        let mut manifest_location = storage.store("./manifest.json")?;
        manifest_location.write_all(manifest_string.as_bytes())?;

        let manifest = manifest.v2();
        if let Some(service::service_manifest_latest::Value::Swagger(swagger)) = &manifest.value {
            handle_openapi(storage, &swagger.source, service.common_api.as_ref(), split)?;
        }

        Ok(())
    }

    /// Writes `credentials` as pretty-printed JSON to `./credentials.json`.
    ///
    /// # Errors
    #[inline]
    pub fn store_credentials<W: io::Write>(
        &self,
        credentials: &Authentication,
        storage: &dyn Storage<W>,
    ) -> error::Result<()> {
        let creds = serde_json::to_string_pretty(credentials)?;

        let mut location = storage.store("./credentials.json")?;
        location.write_all(creds.as_bytes())?;

        Ok(())
    }
}

impl Default for ServiceWriter {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// Reconstructs an `OpenAPI` document (servers, info, paths, component
/// schemas) from `message` and writes it as YAML to `source`, overwriting
/// whatever was already there.
fn handle_openapi<W: io::Write>(
    storage: &dyn Storage<W>,
    source: &str,
    message: Option<&service::CommonApi>,
    _split: bool,
) -> error::Result<()> {
    let default_api = service::CommonApi::default();
    let message = message.unwrap_or(&default_api);

    let mut root = serde_json::Map::new();

    let mut server = serde_json::Map::new();
    server.insert(
        "url".into(),
        message.base_path.clone().unwrap_or_default().into(),
    );
    root.insert("servers".into(), vec![server].into());

    if !message.description.is_empty() || !message.title.is_empty() {
        let mut info = serde_json::Map::new();

        if !message.description.is_empty() {
            info.insert("description".into(), message.description.clone().into());
        }

        if !message.title.is_empty() {
            info.insert("title".into(), message.title.clone().into());
        }

        root.insert("info".into(), info.into());
    }

    let mut paths = serde_json::Map::new();
    handle_path_items(&mut paths, &message.operations)?;
    root.insert("paths".into(), paths.into());

    if !message.schemas.is_empty() {
        let mut components = serde_json::Map::new();
        let mut schemas = serde_json::Map::new();

        for (key, value) in &message.schemas {
            let mut schema = serde_json::Map::new();
            handle_schema(&mut schema, value)?;

            // TODO: handle any path as well as external
            if let Some(key) = key.strip_prefix("#/components/schemas/") {
                schemas.insert(key.into(), schema.into());
            }
        }

        components.insert("schemas".into(), schemas.into());
        root.insert("components".into(), components.into());
    }

    // Serialize and save
    let root_str = serde_yaml::to_string(&root)?;
    let mut storage_location = storage.store(source)?;
    storage_location.write_all(root_str.as_bytes())?;

    Ok(())
}

/// Groups `operations` by their `path`, then by HTTP verb, writing each
/// one's `operationId` and body via [`handle_operation`].
fn handle_path_items(
    paths: &mut serde_json::Map<String, serde_json::Value>,
    operations: &HashMap<String, service::Operation>,
) -> error::Result<()> {
    // TODO: extract into references based on a flag
    for (operation_id, operation) in operations {
        let path_item = paths
            .entry(operation.path.clone())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

        let path_item = path_item
            .as_object_mut()
            .ok_or_else(|| error::ServiceWriter::InvalidType("Object".into()))?;

        let verb = match operation.method {
            service::operation::HttpMethodType::None => {
                return Err(error::ServiceWriter::Unimplemented(
                    "Non Supported HTTP VERB".into(),
                ))
            }
            service::operation::HttpMethodType::Post => "post",
            service::operation::HttpMethodType::Get => "get",
            service::operation::HttpMethodType::Put => "put",
            service::operation::HttpMethodType::Patch => "patch",
            service::operation::HttpMethodType::Delete => "delete",
            service::operation::HttpMethodType::Head => "head",
            service::operation::HttpMethodType::Options => "options",
            service::operation::HttpMethodType::Trace => "trace",
        };

        let path_item = path_item
            .entry(verb)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

        let path_item = path_item
            .as_object_mut()
            .ok_or_else(|| error::ServiceWriter::InvalidType("Object".into()))?;

        path_item.insert("operationId".into(), operation_id.clone().into());
        handle_operation(path_item, operation)?;
    }

    Ok(())
}

/// Writes an operation's summary, description, parameters, request body,
/// and responses into `sink`.
fn handle_operation(
    sink: &mut serde_json::Map<String, serde_json::Value>,
    source: &service::Operation,
) -> error::Result<()> {
    if !source.summary.is_empty() {
        sink.insert("summary".into(), source.summary.clone().into());
    }

    if !source.description.is_empty() {
        sink.insert("description".into(), source.description.clone().into());
    }

    if !source.parameter.is_empty() {
        let mut parameters: Vec<serde_json::Value> = Vec::new();
        for common_param in &source.parameter {
            let mut param = serde_json::Map::new();
            handle_parameter(&mut param, common_param)?;
            parameters.push(param.into());
        }

        sink.insert("parameters".into(), parameters.into());
    }

    if let Some(source_body) = &source.request_body {
        let mut request_body = serde_json::Map::new();
        handle_request_body(&mut request_body, source_body)?;
        sink.insert("requestBody".into(), request_body.into());
    }

    if let Some(common_responses) = &source.api_responses {
        let mut responses = serde_json::Map::new();

        for (status, common_response) in &common_responses.api_responses {
            let mut response = serde_json::Map::new();
            handle_response(&mut response, common_response)?;
            responses.insert(status.clone(), response.into());
        }

        sink.insert("responses".into(), responses.into());
    }

    Ok(())
}

/// Writes a response's content (per MIME type) into `sink`.
fn handle_response(
    sink: &mut serde_json::Map<String, serde_json::Value>,
    source: &service::ApiResponse,
) -> error::Result<()> {
    // TODO: extract into a referece based on a flag

    if !source.content.is_empty() {
        let mut content = serde_json::Map::new();
        for (mime_type, common_media_type) in &source.content {
            let mut media_type = serde_json::Map::new();
            handle_media(&mut media_type, common_media_type)?;
            content.insert(mime_type.clone(), media_type.into());
        }
        sink.insert("content".into(), content.into());
    }

    Ok(())
}

/// Writes a request body's description and content (per MIME type) into
/// `sink`.
fn handle_request_body(
    sink: &mut serde_json::Map<String, serde_json::Value>,
    source: &service::RequestBody,
) -> error::Result<()> {
    // TODO: extract into a referece based on a flag

    if !source.description.is_empty() {
        sink.insert("description".into(), source.description.clone().into());
    }

    if !source.content.is_empty() {
        let mut content = serde_json::Map::new();
        for (mime_type, common_media_type) in &source.content {
            let mut media_type = serde_json::Map::new();
            handle_media(&mut media_type, common_media_type)?;
            content.insert(mime_type.clone(), media_type.into());
        }
        sink.insert("content".into(), content.into());
    }

    Ok(())
}

/// Writes a media type's schema (if any) into `sink`.
fn handle_media(
    sink: &mut serde_json::Map<String, serde_json::Value>,
    source: &service::MediaType,
) -> error::Result<()> {
    if let Some(common_schema) = &source.schema {
        let mut schema = serde_json::Map::new();
        handle_schema(&mut schema, common_schema)?;
        sink.insert("schema".into(), schema.into());
    }

    Ok(())
}

/// Writes a parameter's location, name, requiredness, description, and
/// schema into `sink`. Errors if the parameter's location isn't a
/// recognized [`InType`](service::parameter::InType) variant, rather than
/// writing a bogus `in` value.
fn handle_parameter(
    sink: &mut serde_json::Map<String, serde_json::Value>,
    source: &service::Parameter,
) -> error::Result<()> {
    // TODO: extract into a referece based on a flag

    let in_type = match source.r#in {
        service::parameter::InType::None => {
            return Err(error::ServiceWriter::Unimplemented(
                "Unrecognized parameter location".into(),
            ))
        }
        service::parameter::InType::Query => "query",
        service::parameter::InType::Header => "header",
        service::parameter::InType::Path => "path",
        service::parameter::InType::Cookie => "cookie",
        service::parameter::InType::Headers => "headers",
    };
    sink.insert("in".into(), in_type.into());
    sink.insert("name".into(), source.name.clone().into());
    sink.insert("required".into(), source.required.into());

    if !source.description.is_empty() {
        sink.insert("description".into(), source.description.clone().into());
    }

    if let Some(common_schema) = &source.schema {
        let mut schema = serde_json::Map::new();
        handle_schema(&mut schema, common_schema)?;
        sink.insert("schema".into(), schema.into());
    }

    Ok(())
}

/// Writes a schema into `sink`: a `$ref` string, a typed schema object
/// (recursing into object properties/array items), or an `allOf`/`anyOf`/
/// `oneOf` composition (recursing into each branch). An empty `{}` schema
/// (see [`service::Schema`]'s doc comment) writes nothing into `sink`.
fn handle_schema(
    sink: &mut serde_json::Map<String, serde_json::Value>,
    source: &service::Schema,
) -> error::Result<()> {
    // TODO: extract into a referece based on a flag

    match &source.value {
        Some(service::SchemaValue::Ref(reference)) => {
            sink.insert("$ref".into(), reference.clone().into());
        }
        Some(service::SchemaValue::SchemaObject(schema)) => match schema.r#type {
            service::schema_object::SchemaType::String => {
                sink.insert("type".into(), "string".into());
                // TODO: format???
                // TODO: enum / possibleValues
            }
            service::schema_object::SchemaType::Number => {
                sink.insert("type".into(), "number".into());
            }
            service::schema_object::SchemaType::Integer => {
                sink.insert("type".into(), "integer".into());
            }
            service::schema_object::SchemaType::Boolean => {
                sink.insert("type".into(), "boolean".into());
            }
            service::schema_object::SchemaType::Object => {
                sink.insert("type".into(), "object".into());

                if !schema.properties.is_empty() {
                    let mut properties = serde_json::Map::new();

                    for (key, value) in &schema.properties {
                        let mut prop = serde_json::Map::new();
                        handle_schema(&mut prop, value)?;
                        properties.insert(key.clone(), prop.into());
                    }

                    sink.insert("properties".into(), properties.into());
                }

                if !schema.required.is_empty() {
                    sink.insert("required".into(), schema.required.clone().into());
                }
            }
            service::schema_object::SchemaType::Array => {
                sink.insert("type".into(), "array".into());

                if let Some(common_items) = &schema.items {
                    let mut items = serde_json::Map::new();
                    handle_schema(&mut items, common_items)?;
                    sink.insert("items".into(), items.into());
                }

                // TODO: Max items
            }
            service::schema_object::SchemaType::None => {}
        },
        Some(service::SchemaValue::AllOf(values)) => {
            handle_composed_schema(sink, "allOf", values)?;
        }
        Some(service::SchemaValue::AnyOf(values)) => {
            handle_composed_schema(sink, "anyOf", values)?;
        }
        Some(service::SchemaValue::OneOf(values)) => {
            handle_composed_schema(sink, "oneOf", values)?;
        }
        None => {}
    }

    Ok(())
}

/// Writes a `allOf`/`anyOf`/`oneOf` composition's branches (recursively
/// handled via [`handle_schema`]) into `sink` under `key`.
fn handle_composed_schema(
    sink: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    composed: &service::ComposedSchema,
) -> error::Result<()> {
    let values: error::Result<Vec<serde_json::Value>> = composed
        .schema
        .iter()
        .map(|common_schema| {
            let mut schema = serde_json::Map::new();
            handle_schema(&mut schema, common_schema)?;
            Ok(serde_json::Value::Object(schema))
        })
        .collect();

    sink.insert(key.into(), values?.into());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_parameter_does_not_panic_on_an_unrecognized_location() {
        let source = service::Parameter {
            name: "x".to_owned(),
            r#in: service::parameter::InType::None,
            ..Default::default()
        };

        let mut sink = serde_json::Map::new();
        let result = handle_parameter(&mut sink, &source);

        assert!(
            matches!(result, Err(error::ServiceWriter::Unimplemented(_))),
            "expected an unrecognized parameter location to error instead of writing a bogus \"in\" value, got {result:?}"
        );
    }

    #[test]
    fn handle_path_items_groups_operations_by_path_and_lowercased_verb() {
        let mut operations = HashMap::new();
        operations.insert(
            "getThing".to_owned(),
            service::Operation {
                path: "/thing".to_owned(),
                method: service::operation::HttpMethodType::Get,
                ..Default::default()
            },
        );
        operations.insert(
            "createThing".to_owned(),
            service::Operation {
                path: "/thing".to_owned(),
                method: service::operation::HttpMethodType::Post,
                ..Default::default()
            },
        );

        let mut paths = serde_json::Map::new();
        handle_path_items(&mut paths, &operations).unwrap();

        let path_item = paths.get("/thing").unwrap().as_object().unwrap();
        assert_eq!(
            path_item.get("get").and_then(|op| op.get("operationId")),
            Some(&serde_json::Value::from("getThing"))
        );
        assert_eq!(
            path_item.get("post").and_then(|op| op.get("operationId")),
            Some(&serde_json::Value::from("createThing"))
        );
    }

    #[test]
    fn handle_path_items_rejects_an_unset_http_method() {
        let mut operations = HashMap::new();
        operations.insert(
            "mystery".to_owned(),
            service::Operation {
                path: "/thing".to_owned(),
                ..Default::default()
            },
        );

        let mut paths = serde_json::Map::new();
        let result = handle_path_items(&mut paths, &operations);

        assert!(
            matches!(result, Err(error::ServiceWriter::Unimplemented(_))),
            "expected an unset HTTP method to error, got {result:?}"
        );
    }

    #[test]
    fn handle_schema_writes_composed_schema_branches_under_the_matching_key() {
        let ref_branch =
            |name: &str| service::Schema::new(service::SchemaValue::Ref(name.to_owned()));

        for (value, key) in [
            (
                service::SchemaValue::AllOf(service::ComposedSchema {
                    schema: vec![ref_branch("A"), ref_branch("B")],
                }),
                "allOf",
            ),
            (
                service::SchemaValue::AnyOf(service::ComposedSchema {
                    schema: vec![ref_branch("A"), ref_branch("B")],
                }),
                "anyOf",
            ),
            (
                service::SchemaValue::OneOf(service::ComposedSchema {
                    schema: vec![ref_branch("A"), ref_branch("B")],
                }),
                "oneOf",
            ),
        ] {
            let source = service::Schema::new(value);

            let mut sink = serde_json::Map::new();
            handle_schema(&mut sink, &source).unwrap();

            let branches = sink
                .get(key)
                .unwrap_or_else(|| panic!("expected a \"{key}\" key in {sink:?}"))
                .as_array()
                .unwrap();
            assert_eq!(
                *branches,
                vec![
                    serde_json::json!({ "$ref": "A" }),
                    serde_json::json!({ "$ref": "B" }),
                ]
            );
        }
    }
}
