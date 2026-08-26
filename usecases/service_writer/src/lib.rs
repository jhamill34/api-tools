#![warn(clippy::restriction, clippy::pedantic)]
#![allow(
    clippy::blanket_clippy_restriction_lints,
    clippy::mod_module_files,
    clippy::self_named_module_files,
    clippy::implicit_return,
    clippy::shadow_reuse,
    clippy::match_ref_pats,
    // clippy::shadow_unrelated,
    // clippy::too_many_lines
    clippy::question_mark_used,
    clippy::needless_borrowed_reference,
    clippy::absolute_paths,
    clippy::ref_patterns,
    clippy::single_call_fn
)]

//! Serializes an internal [`VersionedServiceTree`]/[`Authentication`] pair
//! back out to OpenAPI-shaped JSON/YAML and credential JSON, the inverse of
//! `service_loader`'s `OpenAPI` loader.

use std::{collections::HashMap, io};

use core_entities::{service, service::VersionedServiceTree};
use credential_entities::credentials::Authentication;
use protobuf::EnumFull as _;

pub mod error;

/// An output port [`ServiceWriter`] writes to: opens a writable destination
/// for a given `location`.
pub trait Storage<W>
where
    W: io::Write,
{
    /// Opens `location` for writing.
    ///
    /// # Errors
    fn store(&self, location: &str) -> io::Result<W>;
}

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
    /// `./manifest.json.new`, and — if the manifest has an `OpenAPI`
    /// (`swagger`) source — writes the reconstructed `OpenAPI` document
    /// alongside it via `handle_openapi`.
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

        let manifest_string = protobuf_json_mapping::print_to_string(manifest)?;
        let manifest_string: serde_json::Value = serde_json::from_str(&manifest_string)?;
        let manifest_string = serde_json::to_string_pretty(&manifest_string)?;

        let mut manifest_location = storage.store("./manifest.json.new")?;
        manifest_location.write_all(manifest_string.as_bytes())?;

        let manifest = manifest.v2();
        if manifest.has_swagger() {
            let swagger = manifest.swagger();
            handle_openapi(storage, &swagger.source, &service.commonApi, split)?;
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
        let creds = protobuf_json_mapping::print_to_string(credentials)?;

        // Kind of annoying we do this but its just to print it nicely....
        let creds: serde_json::Value = serde_json::from_str(&creds)?;
        let creds = serde_json::to_string_pretty(&creds)?;

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
/// schemas) from `message` and writes it as YAML to `{source}.new`.
fn handle_openapi<W: io::Write>(
    storage: &dyn Storage<W>,
    source: &str,
    message: &service::CommonApi,
    _split: bool,
) -> error::Result<()> {
    let mut root = serde_json::Map::new();

    let mut server = serde_json::Map::new();
    server.insert("url".into(), message.basePath().into());
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
    let source = format!("{source}.new");
    let mut storage_location = storage.store(&source)?;
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

        let verb = match operation.method.enum_value() {
            Ok(service::operation::HttpMethodType::HTTP_METHOD_TYPE_NONE) | Err(_) => {
                return Err(error::ServiceWriter::Unimplemented(
                    "Non Supported HTTP VERB".into(),
                ))
            }
            Ok(method) => method.descriptor().name().to_lowercase(),
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

    if let &Some(ref source_body) = &source.requestBody.0 {
        let mut request_body = serde_json::Map::new();
        handle_request_body(&mut request_body, source_body)?;
        sink.insert("requestBody".into(), request_body.into());
    }

    if let &Some(ref common_responses) = &source.apiResponses.0 {
        let mut responses = serde_json::Map::new();

        for (status, common_response) in &common_responses.apiResponses {
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
    if let &Some(ref common_schema) = &source.schema.0 {
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

    let in_type = source.in_.enum_value().map_err(|_| {
        error::ServiceWriter::Unimplemented("Unrecognized parameter location".into())
    })?;
    sink.insert(
        "in".into(),
        in_type.descriptor().name().to_lowercase().into(),
    );
    sink.insert("name".into(), source.name.clone().into());
    sink.insert("required".into(), source.required.into());

    if !source.description.is_empty() {
        sink.insert("description".into(), source.description.clone().into());
    }

    if let &Some(ref common_schema) = &source.schema.0 {
        let mut schema = serde_json::Map::new();
        handle_schema(&mut schema, common_schema)?;
        sink.insert("schema".into(), schema.into());
    }

    Ok(())
}

/// Writes a schema into `sink`: a `$ref` string, a typed schema object
/// (recursing into object properties/array items), or an `allOf`/`anyOf`/
/// `oneOf` composition (recursing into each branch).
fn handle_schema(
    sink: &mut serde_json::Map<String, serde_json::Value>,
    source: &service::Schema,
) -> error::Result<()> {
    // TODO: extract into a referece based on a flag

    match &source.value {
        &Some(service::schema::Value::Ref(ref reference)) => {
            sink.insert("$ref".into(), reference.clone().into());
        }
        &Some(service::schema::Value::SchemaObject(ref schema)) => {
            match schema.type_.enum_value() {
                Ok(service::schema_object::SchemaType::STRING) => {
                    sink.insert("type".into(), "string".into());
                    // TODO: format???
                    // TODO: enum / possibleValues
                }
                Ok(service::schema_object::SchemaType::NUMBER) => {
                    sink.insert("type".into(), "number".into());
                }
                Ok(service::schema_object::SchemaType::INTEGER) => {
                    sink.insert("type".into(), "integer".into());
                }
                Ok(service::schema_object::SchemaType::BOOLEAN) => {
                    sink.insert("type".into(), "boolean".into());
                }
                Ok(service::schema_object::SchemaType::OBJECT) => {
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
                Ok(service::schema_object::SchemaType::ARRAY) => {
                    sink.insert("type".into(), "array".into());

                    if let &Some(ref common_items) = &schema.items.0 {
                        let mut items = serde_json::Map::new();
                        handle_schema(&mut items, common_items)?;
                        sink.insert("items".into(), items.into());
                    }

                    // TODO: Max items
                }
                _ => {}
            }
        }
        &Some(service::schema::Value::AllOf(ref values)) => {
            handle_composed_schema(sink, "allOf", values)?;
        }
        &Some(service::schema::Value::AnyOf(ref values)) => {
            handle_composed_schema(sink, "anyOf", values)?;
        }
        &Some(service::schema::Value::OneOf(ref values)) => {
            handle_composed_schema(sink, "oneOf", values)?;
        }
        _ => {}
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
    use protobuf::EnumOrUnknown;

    use super::*;

    #[test]
    fn handle_parameter_does_not_panic_on_an_unrecognized_location() {
        let source = service::Parameter {
            name: "x".to_owned(),
            in_: EnumOrUnknown::from_i32(999),
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
                method: service::operation::HttpMethodType::GET.into(),
                ..Default::default()
            },
        );
        operations.insert(
            "createThing".to_owned(),
            service::Operation {
                path: "/thing".to_owned(),
                method: service::operation::HttpMethodType::POST.into(),
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
        let ref_branch = |name: &str| service::Schema {
            value: Some(service::schema::Value::Ref(name.to_owned())),
            ..Default::default()
        };

        for (value, key) in [
            (
                service::schema::Value::AllOf(service::ComposedSchema {
                    schema: vec![ref_branch("A"), ref_branch("B")],
                    ..Default::default()
                }),
                "allOf",
            ),
            (
                service::schema::Value::AnyOf(service::ComposedSchema {
                    schema: vec![ref_branch("A"), ref_branch("B")],
                    ..Default::default()
                }),
                "anyOf",
            ),
            (
                service::schema::Value::OneOf(service::ComposedSchema {
                    schema: vec![ref_branch("A"), ref_branch("B")],
                    ..Default::default()
                }),
                "oneOf",
            ),
        ] {
            let source = service::Schema {
                value: Some(value),
                ..Default::default()
            };

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
