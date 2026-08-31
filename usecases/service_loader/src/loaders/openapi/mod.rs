//! Parses an `OpenAPI` document into a [`service::CommonApi`], recursively
//! walking paths, operations, parameters, request bodies, responses, and
//! schemas, and resolving `$ref` references along the way.

mod utils;

use std::{
    collections::{HashMap, HashSet},
    io,
};

use core_entities::service;

use crate::{error, Fetcher};

use self::utils::{default_field, handle_reference, optional_field, required_field};

/// Fetches and parses the `OpenAPI` document at `source`, converting it into
/// a [`service::CommonApi`]: the base path, title/description, every
/// path's operations, and every referenced schema encountered along the
/// way.
pub fn handle<R: io::Read>(
    fetcher: &dyn Fetcher<R>,
    source: &str,
) -> error::Result<service::CommonApi> {
    let spec = fetcher.fetch(source)?;
    let spec = io::read_to_string(spec)?;

    // NOTE: Big allocation on the Heap here... dropped at the end of this function though...
    let spec: serde_json::Value = serde_yaml::from_str(&spec)?;

    // let seen = HashSet::new();
    let mut cache = HashMap::new();
    let mut schemas = HashMap::new();

    // Convert spec to common api
    let mut api = service::CommonApi::default();
    let server = required_field(&spec, "servers")?;
    let server = get_server(&server)?;
    api.base_path = Some(server);

    if let Some(info) = spec.get("info") {
        if let Some(description) = optional_field::<String>(info, "description")? {
            api.description = description;
        }

        if let Some(title) = optional_field::<String>(info, "title")? {
            api.title = title;
        }
    }

    let paths: HashMap<String, serde_json::Value> = default_field(&spec, "paths")?;
    for (path, item) in paths {
        api.operations.extend(collect_operations(
            &path,
            &item,
            &spec,
            fetcher,
            &mut cache,
            &mut schemas,
        )?);
    }

    api.schemas = schemas;

    Ok(api)
}

/// Extracts the first entry's `url` from an `OpenAPI` `servers` array.
fn get_server(server: &serde_json::Value) -> error::Result<String> {
    // server.get(0).map(|s| s.url.clone()).ok_or(error::ServiceLoaderError::NotFound("Server".to_string()))
    let server = server
        .get(0)
        .ok_or(error::ServiceLoader::NotFound("Server".into()))?;
    required_field(server, "url")
}

/// The `OpenAPI` path-item verbs [`collect_operations`] recognizes, paired
/// with the [`service::operation::HttpMethodType`] each one maps to.
const HTTP_METHODS: &[(&str, service::operation::HttpMethodType)] = &[
    ("get", service::operation::HttpMethodType::Get),
    ("post", service::operation::HttpMethodType::Post),
    ("put", service::operation::HttpMethodType::Put),
    ("patch", service::operation::HttpMethodType::Patch),
    ("delete", service::operation::HttpMethodType::Delete),
    ("head", service::operation::HttpMethodType::Head),
    ("options", service::operation::HttpMethodType::Options),
    ("trace", service::operation::HttpMethodType::Trace),
];

/// Resolves `item` (a path item, possibly a `$ref`) and converts each
/// HTTP-verb entry it defines (get/post/put/patch/delete/head/options/
/// trace) into an `(operationId, Operation)` pair via [`handle_operation`],
/// sharing `item`'s path-level parameters across all of them.
fn collect_operations<R: io::Read>(
    path: &str,
    item: &serde_json::Value,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
) -> error::Result<Vec<(String, service::Operation)>> {
    let reference = handle_reference(item, root, fetcher, cache, &mut HashSet::new())?;
    let item = reference.as_ref().map_or(item, |(_, item)| item);

    let parameters: Vec<serde_json::Value> = default_field(item, "parameters")?;
    let mut common_params = vec![];
    for param in parameters {
        let mut common_param = service::Parameter::default();
        handle_parameter(&param, &mut common_param, root, fetcher, cache, schemas)?;
        common_params.push(common_param);
    }

    let mut result = Vec::new();

    for &(verb, method) in HTTP_METHODS {
        if let Some(op) = item.get(verb) {
            let mut common_op = service::Operation {
                path: path.to_owned(),
                method,
                ..Default::default()
            };
            handle_operation(
                op,
                &mut common_op,
                root,
                fetcher,
                cache,
                schemas,
                &common_params,
            )?;
            result.push((required_field(op, "operationId")?, common_op));
        }
    }

    Ok(result)
}

/// Converts an operation's summary, description, parameters (merged with
/// any path-level `common_params`), request body, responses, and
/// `x-pagination` extension into `sink`.
fn handle_operation<R: io::Read>(
    source: &serde_json::Value,
    sink: &mut service::Operation,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
    common_params: &[service::Parameter],
) -> error::Result<()> {
    if let Some(summary) = optional_field::<String>(source, "summary")? {
        sink.summary = summary;
    }

    if let Some(description) = optional_field::<String>(source, "description")? {
        sink.description = description;
    }

    sink.parameter.extend_from_slice(common_params);
    let parameters: Vec<serde_json::Value> = default_field(source, "parameters")?;
    for param in parameters {
        let mut common_param = service::Parameter::default();
        handle_parameter(&param, &mut common_param, root, fetcher, cache, schemas)?;
        sink.parameter.push(common_param);
    }

    if let Some(request_body) = source.get("requestBody") {
        let mut common_request_body = service::RequestBody::default();
        handle_request_body(
            request_body,
            &mut common_request_body,
            root,
            fetcher,
            cache,
            schemas,
        )?;
        sink.request_body = Some(common_request_body);
    }

    let responses: HashMap<String, serde_json::Value> = default_field(source, "responses")?;
    if !responses.is_empty() {
        let mut common_responses = service::ApiResponses::default();
        for (status, response) in &responses {
            let mut common_response = service::ApiResponse::default();
            handle_response(
                response,
                &mut common_response,
                root,
                fetcher,
                cache,
                schemas,
            )?;
            common_responses
                .api_responses
                .insert(status.clone(), common_response);
        }
        sink.api_responses = Some(common_responses);
    }

    if let Some(pagination) = source.get("x-pagination") {
        sink.pagination = Some(handle_pagination(pagination)?);
    }

    Ok(())
}

/// Converts an operation's `x-pagination` extension, picking the pagination
/// strategy (page-offset, offset, next-URL, cursor, or unpaginated) from
/// whichever of `pageOffset`/`offset`/`nextUrl`/`cursor` is present in
/// `source`.
fn handle_pagination(source: &serde_json::Value) -> error::Result<service::Pagination> {
    let results_path =
        service::pagination::ExtendedPath::JmesPath(default_field(source, "resultsPath")?);

    let pagination = if let Some(page_offset) = source.get("pageOffset") {
        service::Pagination::PageOffset(service::pagination::PageOffset {
            page_offset_param: default_field(page_offset, "pageOffsetParam")?,
            start_page: Some(default_field::<i32>(page_offset, "startPage")?),
            limit_param: default_field(page_offset, "limitParam")?,
            max_limit: Some(default_field::<i32>(page_offset, "maxLimit")?),
            results_path: Some(results_path),
            error_on_path_not_found: None,
        })
    } else if let Some(offset) = source.get("offset") {
        service::Pagination::Offset(service::pagination::Offset {
            offset_param: default_field(offset, "offsetParam")?,
            limit_param: default_field(offset, "limitParam")?,
            max_limit: Some(default_field::<i32>(offset, "maxLimit")?),
            results_path: Some(results_path),
            error_on_path_not_found: None,
        })
    } else if let Some(next_url) = source.get("nextUrl") {
        let next_url_path =
            service::pagination::ExtendedPath::JmesPath(default_field(next_url, "nextUrlPath")?);

        service::Pagination::NextUrl(service::pagination::NextUrl {
            next_url_path: Some(next_url_path),
            limit_param: default_field(next_url, "limitParam")?,
            max_limit: Some(default_field::<i32>(next_url, "maxLimit")?),
            results_path: Some(results_path),
            error_on_path_not_found: None,
        })
    } else if let Some(cursor) = source.get("cursor") {
        let cursor_path =
            service::pagination::ExtendedPath::JmesPath(default_field(cursor, "cursorPath")?);

        service::Pagination::MultiCursor(service::pagination::MultiCursor {
            cursors_path: vec![cursor_path],
            cursors_param: vec![default_field::<String>(cursor, "cursorParam")?],
            limit_param: default_field(cursor, "limitParam")?,
            max_limit: Some(default_field::<i32>(cursor, "maxLimit")?),
            results_path: Some(results_path),
            error_on_path_not_found: None,
        })
    } else {
        service::Pagination::Unpaginated(service::pagination::Unpaginated {
            results_path: Some(results_path),
            error_on_path_not_found: None,
        })
    };

    Ok(pagination)
}

/// Resolves `source` (possibly a `$ref`) and converts its location
/// (`header`/`query`/`path`/`cookie`), name, requiredness, description,
/// and schema into `sink`. An unrecognized location becomes
/// [`None`](service::parameter::InType::None) rather than erroring.
fn handle_parameter<R: io::Read>(
    source: &serde_json::Value,
    sink: &mut service::Parameter,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
) -> error::Result<()> {
    let reference = handle_reference(source, root, fetcher, cache, &mut HashSet::new())?;
    let source = reference.as_ref().map_or(source, |(_, item)| item);

    let in_ = required_field::<String>(source, "in")?;
    let in_ = match in_.as_str() {
        "header" => service::parameter::InType::Header,
        "query" => service::parameter::InType::Query,
        "path" => service::parameter::InType::Path,
        "cookie" => service::parameter::InType::Cookie,
        _ => service::parameter::InType::None,
    };
    sink.r#in = in_;

    sink.name = required_field(source, "name")?;
    sink.required = default_field(source, "required")?;

    if let Some(description) = optional_field(source, "description")? {
        sink.description = description;
    }

    if let Some(schema) = source.get("schema") {
        let mut common_schema = service::Schema::default();
        handle_schema(schema, &mut common_schema, root, fetcher, cache, schemas)?;
        sink.schema = Some(common_schema);
    }

    Ok(())
}

/// Resolves `source` (possibly a `$ref`) and converts its description and
/// per-MIME-type content into `sink`.
fn handle_request_body<R: io::Read>(
    source: &serde_json::Value,
    sink: &mut service::RequestBody,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
) -> error::Result<()> {
    let reference = handle_reference(source, root, fetcher, cache, &mut HashSet::new())?;
    let source = reference.as_ref().map_or(source, |(_, item)| item);

    if let Some(description) = optional_field(source, "description")? {
        sink.description = description;
    }

    let content: HashMap<String, serde_json::Value> = default_field(source, "content")?;
    for (key, value) in &content {
        let mut common_media_type = service::MediaType::default();
        handle_media_type(value, &mut common_media_type, root, fetcher, cache, schemas)?;
        sink.content.insert(key.clone(), common_media_type);
    }

    Ok(())
}

/// Resolves `source` (possibly a `$ref`) and converts its per-MIME-type
/// content into `sink`. Response headers aren't represented in the schema,
/// so they're not converted.
fn handle_response<R: io::Read>(
    source: &serde_json::Value,
    sink: &mut service::ApiResponse,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
) -> error::Result<()> {
    let reference = handle_reference(source, root, fetcher, cache, &mut HashSet::new())?;
    let source = reference.as_ref().map_or(source, |(_, item)| item);

    let content: HashMap<String, serde_json::Value> = default_field(source, "content")?;
    for (key, value) in &content {
        let mut common_media_type = service::MediaType::default();
        handle_media_type(value, &mut common_media_type, root, fetcher, cache, schemas)?;
        sink.content.insert(key.clone(), common_media_type);
    }

    // NOTE: Response Headers aren't included in the schema

    Ok(())
}

/// Converts a media type's schema (if any) into `sink`.
fn handle_media_type<R: io::Read>(
    source: &serde_json::Value,
    sink: &mut service::MediaType,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
) -> error::Result<()> {
    if let Some(schema) = source.get("schema") {
        let mut common_schema = service::Schema::default();
        handle_schema(schema, &mut common_schema, root, fetcher, cache, schemas)?;
        sink.schema = Some(common_schema);
    }

    Ok(())
}

/// Converts a schema into `sink`. If `source` is a `$ref` (including one
/// that resolves to a reference cycle), records it by key in `schemas` and
/// sets `sink` to a `$ref` pointer instead of inlining it. Otherwise
/// converts `source`'s recognized `type` (string/boolean/integer/number,
/// recursing into array items or object properties) or, absent a `type`,
/// its `oneOf`/`anyOf`/`allOf` composition (recursing into each branch), or
/// - absent both - leaves `sink` as an empty `{}` schema.
fn handle_schema<R: io::Read>(
    source: &serde_json::Value,
    sink: &mut service::Schema,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
) -> error::Result<()> {
    let reference = handle_reference(source, root, fetcher, cache, &mut HashSet::new());
    if let Err(error::ServiceLoader::CyclicalReference(key)) = reference {
        sink.value = Some(service::SchemaValue::Ref(key));
        return Ok(());
    }
    let reference = reference?;

    if let Some((key, source)) = reference {
        sink.value = Some(service::SchemaValue::Ref(key.clone()));

        if !schemas.contains_key(&key) {
            // Placeholder to break a reference cycle - its value is never
            // read before being overwritten below, only its presence as a
            // key is checked.
            schemas.insert(key.clone(), service::Schema::default());
            let mut ref_type = service::Schema::default();
            handle_schema(&source, &mut ref_type, root, fetcher, cache, schemas)?;
            schemas.insert(key, ref_type);
        }
        return Ok(());
    }

    let type_ = optional_field::<String>(source, "type")?;

    if let Some(type_) = type_ {
        match type_.as_str() {
            "string" => {
                sink.value = Some(service::SchemaValue::SchemaObject(service::SchemaObject {
                    r#type: service::schema_object::SchemaType::String,
                    ..Default::default()
                }));
            }
            "boolean" => {
                sink.value = Some(service::SchemaValue::SchemaObject(service::SchemaObject {
                    r#type: service::schema_object::SchemaType::Boolean,
                    ..Default::default()
                }));
            }
            "integer" => {
                sink.value = Some(service::SchemaValue::SchemaObject(service::SchemaObject {
                    r#type: service::schema_object::SchemaType::Integer,
                    ..Default::default()
                }));
            }
            "number" => {
                sink.value = Some(service::SchemaValue::SchemaObject(service::SchemaObject {
                    r#type: service::schema_object::SchemaType::Number,
                    ..Default::default()
                }));
            }
            "array" => {
                let items = if let Some(items) = source.get("items") {
                    let mut common_items = service::Schema::default();
                    handle_schema(items, &mut common_items, root, fetcher, cache, schemas)?;
                    Some(Box::new(common_items))
                } else {
                    None
                };

                sink.value = Some(service::SchemaValue::SchemaObject(service::SchemaObject {
                    r#type: service::schema_object::SchemaType::Array,
                    items,
                    ..Default::default()
                }));
            }
            "object" => {
                let properties: HashMap<String, serde_json::Value> =
                    default_field(source, "properties")?;

                let properties: error::Result<HashMap<String, service::Schema>> = properties
                    .iter()
                    .map(|(key, value)| {
                        let mut common_property = service::Schema::default();
                        handle_schema(value, &mut common_property, root, fetcher, cache, schemas)?;
                        Ok((key.clone(), common_property))
                    })
                    .collect();
                let properties = properties?;

                let required: Vec<String> = default_field(source, "required")?;
                sink.value = Some(service::SchemaValue::SchemaObject(service::SchemaObject {
                    r#type: service::schema_object::SchemaType::Object,
                    properties,
                    required,
                    ..Default::default()
                }));
            }
            _ => {}
        }
    } else {
        if let Some(schema) = resolve_schema_list(source, "oneOf", root, fetcher, cache, schemas)? {
            sink.value = Some(service::SchemaValue::OneOf(service::ComposedSchema {
                schema,
            }));
        }

        if let Some(schema) = resolve_schema_list(source, "anyOf", root, fetcher, cache, schemas)? {
            sink.value = Some(service::SchemaValue::AnyOf(service::ComposedSchema {
                schema,
            }));
        }

        if let Some(schema) = resolve_schema_list(source, "allOf", root, fetcher, cache, schemas)? {
            sink.value = Some(service::SchemaValue::AllOf(service::ComposedSchema {
                schema,
            }));
        }
    }

    Ok(())
}

/// Resolves `source`'s `field` (`"oneOf"`/`"anyOf"`/`"allOf"`) as a list of
/// schemas, recursively converting each branch via [`handle_schema`].
/// Returns `None` if `field` is absent - the shared body of
/// [`handle_schema`]'s three composition-field arms.
fn resolve_schema_list<R: io::Read>(
    source: &serde_json::Value,
    field: &str,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
) -> error::Result<Option<Vec<service::Schema>>> {
    let Some(values) = optional_field::<Vec<serde_json::Value>>(source, field)? else {
        return Ok(None);
    };

    values
        .iter()
        .map(|value| {
            let mut common_schema = service::Schema::default();
            handle_schema(value, &mut common_schema, root, fetcher, cache, schemas)?;
            Ok(common_schema)
        })
        .collect::<error::Result<Vec<_>>>()
        .map(Some)
}

#[cfg(test)]
mod test {
    use core::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct SimpleFetcher {
        docs: HashMap<String, String>,
        counts: RefCell<HashMap<String, u8>>,
    }

    impl SimpleFetcher {
        fn new() -> Self {
            Self {
                docs: HashMap::new(),
                counts: RefCell::new(HashMap::new()),
            }
        }

        fn with(self, name: &str, doc: &str) -> Self {
            let mut fetcher = self;
            fetcher.docs.insert(name.to_owned(), doc.to_owned());
            fetcher.counts.borrow_mut().insert(name.to_owned(), 0);
            fetcher
        }
    }

    impl Fetcher<io::Cursor<Vec<u8>>> for SimpleFetcher {
        fn fetch(&self, location: &str) -> io::Result<io::Cursor<Vec<u8>>> {
            let doc = self.docs.get(location).expect("Expected document to exist");
            let c = io::Cursor::new(doc.as_bytes().to_vec());

            if let Some(count) = self.counts.borrow_mut().get_mut(location) {
                *count += 1;
            }

            Ok(c)
        }
    }

    #[test]
    fn test_basic_root() -> error::Result<()> {
        let doc = include_str!("stubs/basic_root.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        assert_eq!("example description", root.description);
        assert_eq!("Example API", root.title);
        assert_eq!(Some("https://example.com".to_owned()), root.base_path);

        Ok(())
    }

    #[test]
    fn test_basic_path() -> error::Result<()> {
        let doc = include_str!("stubs/basic_path.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let operation = root.operations.get("say_hello").unwrap();
        assert_eq!(service::operation::HttpMethodType::Get, operation.method);
        assert_eq!("/hello", operation.path);

        Ok(())
    }

    #[test]
    fn test_path_item_parameters() -> error::Result<()> {
        let doc = include_str!("stubs/path_item_parameters.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();
        assert_eq!(1, op.parameter.len());
        let param = &op.parameter[0];

        assert_eq!(service::parameter::InType::Header, param.r#in);
        assert_eq!("Version", param.name);
        assert!(!param.required);

        Ok(())
    }

    #[test]
    fn test_path_item_parameters_ref() -> error::Result<()> {
        let doc = include_str!("stubs/path_item_parameters_ref.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();
        assert_eq!(1, op.parameter.len());
        let param = &op.parameter[0];

        assert_eq!(service::parameter::InType::Header, param.r#in);
        assert_eq!("Version", param.name);
        assert!(!param.required);

        Ok(())
    }

    #[test]
    fn test_basic_path_with_ref() -> error::Result<()> {
        let doc = include_str!("stubs/basic_path_with_ref.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        root.operations.get("say_hello").unwrap();

        Ok(())
    }

    #[test]
    fn test_path_item_request_body() -> error::Result<()> {
        let doc = include_str!("stubs/path_item_request_body.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let path = root.operations.get("say_hello").unwrap();

        let req_body = path.request_body.as_ref().unwrap();
        assert_eq!("Say your thing", req_body.description);
        assert!(!req_body.required);
        assert_eq!(1, req_body.content.len());

        Ok(())
    }

    #[test]
    fn test_path_item_request_body_ref() -> error::Result<()> {
        let doc = include_str!("stubs/path_item_request_body_ref.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let path = root.operations.get("say_hello").unwrap();

        let req_body = path.request_body.as_ref().unwrap();
        assert_eq!("Say your thing", req_body.description);
        assert!(!req_body.required);
        assert_eq!(1, req_body.content.len());

        Ok(())
    }

    #[test]
    fn test_path_item_responses() -> error::Result<()> {
        let doc = include_str!("stubs/path_item_responses.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();

        let ok_response = op
            .api_responses
            .as_ref()
            .unwrap()
            .api_responses
            .get("200")
            .unwrap();
        assert_eq!(1, ok_response.content.len());

        Ok(())
    }

    #[test]
    fn test_path_item_responses_ref() -> error::Result<()> {
        let doc = include_str!("stubs/path_item_responses_ref.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();

        let ok_response = op
            .api_responses
            .as_ref()
            .unwrap()
            .api_responses
            .get("200")
            .unwrap();
        assert_eq!(1, ok_response.content.len());

        Ok(())
    }

    #[test]
    fn test_path_item_pagination() -> error::Result<()> {
        let doc = include_str!("stubs/path_item_pagination.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();

        let page = op.pagination.as_ref().unwrap();
        let service::Pagination::Offset(page) = page else {
            panic!("expected an Offset pagination strategy, got {page:?}");
        };

        assert_eq!(Some(100), page.max_limit);
        let Some(service::pagination::ExtendedPath::JmesPath(results_path)) = &page.results_path
        else {
            panic!(
                "expected a JmesPath resultsPath, got {:?}",
                page.results_path
            );
        };
        assert_eq!("$response.body#/", results_path);

        Ok(())
    }

    #[test]
    fn test_basic_schema() -> error::Result<()> {
        let doc = include_str!("stubs/basic_schema.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();

        assert_eq!(1, op.parameter.len());
        let param = &op.parameter[0];

        let schema = param.schema.as_ref().unwrap();
        let Some(service::SchemaValue::SchemaObject(schema)) = &schema.value else {
            panic!("expected a SchemaObject, got {schema:?}");
        };
        assert_eq!(service::schema_object::SchemaType::String, schema.r#type);

        Ok(())
    }

    #[test]
    fn test_array_schema() -> error::Result<()> {
        let doc = include_str!("stubs/array_schema.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();

        let param = &op.parameter[0];

        let schema = param.schema.as_ref().unwrap();
        let Some(service::SchemaValue::SchemaObject(schema)) = &schema.value else {
            panic!("expected a SchemaObject, got {schema:?}");
        };

        assert_eq!(service::schema_object::SchemaType::Array, schema.r#type);

        let items = schema.items.as_ref().unwrap();
        let Some(service::SchemaValue::SchemaObject(items)) = &items.value else {
            panic!("expected a SchemaObject, got {items:?}");
        };
        assert_eq!(service::schema_object::SchemaType::String, items.r#type);

        Ok(())
    }

    #[test]
    fn test_object_schema() -> error::Result<()> {
        let doc = include_str!("stubs/object_schema.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();

        let param = &op.parameter[0];

        let schema = param.schema.as_ref().unwrap();
        let Some(service::SchemaValue::SchemaObject(schema)) = &schema.value else {
            panic!("expected a SchemaObject, got {schema:?}");
        };

        assert_eq!(service::schema_object::SchemaType::Object, schema.r#type);

        let props = &schema.properties;
        let Some(service::SchemaValue::SchemaObject(foo)) = &props.get("foo").unwrap().value else {
            panic!("expected a SchemaObject");
        };
        assert_eq!(service::schema_object::SchemaType::Number, foo.r#type);

        let Some(service::SchemaValue::SchemaObject(bar)) = &props.get("bar").unwrap().value else {
            panic!("expected a SchemaObject");
        };
        assert_eq!(service::schema_object::SchemaType::Object, bar.r#type);

        let Some(service::SchemaValue::SchemaObject(baz)) =
            &bar.properties.get("baz").unwrap().value
        else {
            panic!("expected a SchemaObject");
        };
        assert_eq!(service::schema_object::SchemaType::String, baz.r#type);

        Ok(())
    }

    #[test]
    fn test_oneof_schema() -> error::Result<()> {
        let doc = include_str!("stubs/oneof_schema.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();

        let param = &op.parameter[0];

        let schema = param.schema.as_ref().unwrap();
        let Some(service::SchemaValue::OneOf(schema)) = &schema.value else {
            panic!("expected a OneOf schema, got {schema:?}");
        };
        let schema = &schema.schema;

        let Some(service::SchemaValue::SchemaObject(first)) = &schema[0].value else {
            panic!("expected a SchemaObject");
        };
        assert_eq!(service::schema_object::SchemaType::String, first.r#type);
        let Some(service::SchemaValue::SchemaObject(second)) = &schema[1].value else {
            panic!("expected a SchemaObject");
        };
        assert_eq!(service::schema_object::SchemaType::Number, second.r#type);

        Ok(())
    }

    #[test]
    fn test_basic_path_with_double_ref() -> error::Result<()> {
        let doc = include_str!("stubs/basic_path_with_double_ref.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();
        let param = &op.parameter[0];
        assert_eq!("Version", param.name);

        let op = root.operations.get("post_hello").unwrap();
        let param = &op.parameter[0];
        assert_eq!("Version", param.name);

        Ok(())
    }

    #[test]
    fn test_basic_path_with_ref_cycle() -> error::Result<()> {
        let doc = include_str!("stubs/basic_path_with_ref_cycle.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main");

        assert!(matches!(
            root,
            Err(error::ServiceLoader::CyclicalReference(_))
        ));

        Ok(())
    }

    const REMOTE_DOC: &str = include_str!("stubs/remote_doc.yaml");

    #[test]
    fn test_basic_path_with_remote_ref() -> error::Result<()> {
        let doc = include_str!("stubs/basic_path_with_remote_ref.yaml");
        let fetcher = SimpleFetcher::new()
            .with("main", doc)
            .with("https://example.com/json", REMOTE_DOC);

        let root = handle(&fetcher, "main")?;
        root.operations.get("say_hello").unwrap();

        assert_eq!(
            1,
            *fetcher
                .counts
                .borrow()
                .get("https://example.com/json")
                .unwrap()
        );
        Ok(())
    }

    #[test]
    fn test_basic_path_with_local_ref() -> error::Result<()> {
        let doc = include_str!("stubs/basic_path_with_local_ref.yaml");

        let fetcher = SimpleFetcher::new()
            .with("main", doc)
            .with("./test.json", REMOTE_DOC);

        let root = handle(&fetcher, "main")?;
        root.operations.get("say_hello").unwrap();

        assert_eq!(1, *fetcher.counts.borrow().get("./test.json").unwrap());

        Ok(())
    }

    #[test]
    fn test_basic_path_with_double_remote_ref() -> error::Result<()> {
        let doc = include_str!("stubs/basic_path_with_double_remote_ref.yaml");

        let fetcher = SimpleFetcher::new()
            .with("main", doc)
            .with("./test.json", REMOTE_DOC);

        let root = handle(&fetcher, "main")?;
        root.operations.get("say_hello").unwrap();

        assert_eq!(1, *fetcher.counts.borrow().get("./test.json").unwrap());

        Ok(())
    }

    #[test]
    fn test_basic_schema_with_ref() -> error::Result<()> {
        let doc = include_str!("stubs/basic_schema_with_ref.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;
        let op = root.operations.get("say_hello").unwrap();

        let param = &op.parameter[0];
        assert_eq!(service::parameter::InType::Header, param.r#in);
        assert_eq!("Version", param.name);
        assert!(!param.required);

        let schema = param.schema.as_ref().unwrap();
        let Some(service::SchemaValue::Ref(schema)) = &schema.value else {
            panic!("expected a Ref, got {schema:?}");
        };
        assert_eq!("#/components/schemas/Version", schema);

        let schema = root.schemas.get("#/components/schemas/Version").unwrap();
        let Some(service::SchemaValue::SchemaObject(schema)) = &schema.value else {
            panic!("expected a SchemaObject, got {schema:?}");
        };
        assert_eq!(service::schema_object::SchemaType::String, schema.r#type);

        Ok(())
    }

    #[test]
    fn test_schema_with_ref_cycle() -> error::Result<()> {
        let doc = include_str!("stubs/schema_with_ref_cycle.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;
        let op = root.operations.get("say_hello").unwrap();

        let param = &op.parameter[0];
        assert_eq!(service::parameter::InType::Header, param.r#in);
        assert_eq!("Version", param.name);
        assert!(!param.required);

        let schema = param.schema.as_ref().unwrap();
        let Some(service::SchemaValue::Ref(schema)) = &schema.value else {
            panic!("expected a Ref, got {schema:?}");
        };
        assert_eq!("#/components/schemas/OtherVersion", schema);

        let schema = root
            .schemas
            .get("#/components/schemas/OtherVersion")
            .unwrap();
        let Some(service::SchemaValue::SchemaObject(schema_obj)) = &schema.value else {
            panic!("expected a SchemaObject, got {schema:?}");
        };
        assert_eq!(
            service::schema_object::SchemaType::Object,
            schema_obj.r#type
        );

        let foo = schema_obj.properties.get("foo").unwrap();
        let Some(service::SchemaValue::Ref(foo)) = &foo.value else {
            panic!("expected a Ref, got {foo:?}");
        };
        assert_eq!("#/components/schemas/OtherVersion", foo);

        Ok(())
    }

    #[test]
    fn test_oneof_schema_with_ref() -> error::Result<()> {
        let doc = include_str!("stubs/oneof_schema_with_ref.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;
        let op = root.operations.get("say_hello").unwrap();

        let param = &op.parameter[0];
        assert_eq!(service::parameter::InType::Header, param.r#in);
        assert_eq!("Version", param.name);
        assert!(!param.required);

        let schema = param.schema.as_ref().unwrap();
        let Some(service::SchemaValue::OneOf(schemas)) = &schema.value else {
            panic!("expected a OneOf, got {schema:?}");
        };

        let Some(service::SchemaValue::SchemaObject(first)) = &schemas.schema[0].value else {
            panic!("expected a SchemaObject");
        };
        assert_eq!(service::schema_object::SchemaType::String, first.r#type);

        let Some(service::SchemaValue::Ref(second)) = &schemas.schema[1].value else {
            panic!("expected a Ref");
        };
        assert_eq!("#/components/schemas/Number", second);

        let schema = root.schemas.get("#/components/schemas/Number").unwrap();
        let Some(service::SchemaValue::SchemaObject(schema)) = &schema.value else {
            panic!("expected a SchemaObject, got {schema:?}");
        };
        assert_eq!(service::schema_object::SchemaType::Number, schema.r#type);

        Ok(())
    }
}
