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
)]

//!

use std::{collections::HashMap, io};

use core_entities::{service, service::VersionedServiceTree};
use credential_entities::credentials::Authentication;
use protobuf::EnumFull as _;

pub mod error;

///
pub trait Storage<W>
where
    W: io::Write,
{
    ///
    /// # Errors
    fn store(&self, location: &str) -> io::Result<W>;
}

///
#[non_exhaustive]
pub struct ServiceWriter;

impl ServiceWriter {
    ///
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self
    }

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

///
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

///
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
        let path_item = match operation.method.enum_value() {
            Ok(service::operation::HttpMethodType::GET) => path_item
                .entry(String::from("get"))
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new())),
            Ok(service::operation::HttpMethodType::POST) => path_item
                .entry(String::from("post"))
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new())),
            Ok(service::operation::HttpMethodType::PUT) => path_item
                .entry(String::from("put"))
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new())),
            Ok(service::operation::HttpMethodType::PATCH) => path_item
                .entry(String::from("patch"))
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new())),
            Ok(service::operation::HttpMethodType::DELETE) => path_item
                .entry(String::from("delete"))
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new())),
            Ok(service::operation::HttpMethodType::HEAD) => path_item
                .entry(String::from("head"))
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new())),
            Ok(service::operation::HttpMethodType::OPTIONS) => path_item
                .entry(String::from("options"))
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new())),
            Ok(service::operation::HttpMethodType::TRACE) => path_item
                .entry(String::from("trace"))
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new())),
            Ok(service::operation::HttpMethodType::HTTP_METHOD_TYPE_NONE) | Err(_) => {
                return Err(error::ServiceWriter::Unimplemented(
                    "Non Supported HTTP VERB".into(),
                ))
            }
        };

        let path_item = path_item
            .as_object_mut()
            .ok_or_else(|| error::ServiceWriter::InvalidType("Object".into()))?;

        path_item.insert("operationId".into(), operation_id.clone().into());
        handle_operation(path_item, operation)?;
    }

    Ok(())
}

///
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

///
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

///
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

///
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

///
fn handle_parameter(
    sink: &mut serde_json::Map<String, serde_json::Value>,
    source: &service::Parameter,
) -> error::Result<()> {
    // TODO: extract into a referece based on a flag

    sink.insert(
        "in".into(),
        source
            .in_
            .unwrap()
            .descriptor()
            .name()
            .to_lowercase()
            .into(),
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

///
fn handle_schema(
    sink: &mut serde_json::Map<String, serde_json::Value>,
    source: &service::Schema,
) -> error::Result<()> {
    // TODO: extract into a referece based on a flag

    match &source.value {
        &Some(service::schema::Value::Ref(ref r)) => {
            sink.insert("$ref".into(), r.clone().into());
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
            let values: error::Result<Vec<serde_json::Value>> = values
                .schema
                .iter()
                .map(|common_schema| {
                    let mut schema = serde_json::Map::new();
                    handle_schema(&mut schema, common_schema)?;
                    Ok(serde_json::Value::Object(schema))
                })
                .collect();

            sink.insert("allOf".into(), values?.into());
        }
        &Some(service::schema::Value::AnyOf(ref values)) => {
            let values: error::Result<Vec<serde_json::Value>> = values
                .schema
                .iter()
                .map(|common_schema| {
                    let mut schema = serde_json::Map::new();
                    handle_schema(&mut schema, common_schema)?;
                    Ok(serde_json::Value::Object(schema))
                })
                .collect();

            sink.insert("anyOf".into(), values?.into());
        }
        &Some(service::schema::Value::OneOf(ref values)) => {
            let values: error::Result<Vec<serde_json::Value>> = values
                .schema
                .iter()
                .map(|common_schema| {
                    let mut schema = serde_json::Map::new();
                    handle_schema(&mut schema, common_schema)?;
                    Ok(serde_json::Value::Object(schema))
                })
                .collect();

            sink.insert("oneOf".into(), values?.into());
        }
        _ => {}
    }

    Ok(())
}
