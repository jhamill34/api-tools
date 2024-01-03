//!

mod utils;

use std::{
    collections::{HashMap, HashSet},
    io,
};

use crate::{error, Fetcher};
use core_entities::service;

use self::utils::{default_field, handle_reference, optional_field, required_field};

///
pub fn handle<R: io::Read>(
    fetcher: &dyn Fetcher<R>,
    source: &str,
) -> error::Result<protobuf::MessageField<service::CommonApi>> {
    let spec = fetcher.fetch(source)?;
    let spec = io::read_to_string(spec)?;

    // NOTE: Big allocation on the Heap here... dropped at the end of this function though...
    let spec: serde_json::Value = serde_yaml::from_str(&spec)?;

    // let seen = HashSet::new();
    let mut cache = HashMap::new();
    let mut schemas = HashMap::new();

    // Convert spec to common api
    let mut api = service::CommonApi::new();
    let server = required_field(&spec, "servers")?;
    let server = get_server(&server)?;
    api.set_basePath(server);

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

    Ok(protobuf::MessageField::some(api))
}

///
fn get_server(server: &serde_json::Value) -> error::Result<String> {
    // server.get(0).map(|s| s.url.clone()).ok_or(error::ServiceLoaderError::NotFound("Server".to_string()))
    let server = server
        .get(0)
        .ok_or(error::ServiceLoader::NotFound("Server".into()))?;
    required_field(server, "url")
}

///
fn collect_operations<R: io::Read>(
    path: &str,
    item: &serde_json::Value,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
) -> error::Result<Vec<(String, service::Operation)>> {
    let reference = handle_reference(item, root, fetcher, cache, &mut HashSet::new())?;
    let item = reference.as_ref().map_or(item, |&(_, ref item)| item);

    let parameters: Vec<serde_json::Value> = default_field(item, "parameters")?;
    let mut common_params = vec![];
    for param in parameters {
        let mut common_param = service::Parameter::new();
        handle_parameter(&param, &mut common_param, root, fetcher, cache, schemas)?;
        common_params.push(common_param);
    }

    let mut result = Vec::new();

    if let Some(op) = item.get("get") {
        let mut common_op = service::Operation::new();
        common_op.path = path.to_owned();
        common_op.method = service::operation::HttpMethodType::GET.into();
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

    if let Some(op) = item.get("post") {
        let mut common_op = service::Operation::new();
        common_op.path = path.to_owned();
        common_op.method = service::operation::HttpMethodType::POST.into();
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

    if let Some(op) = item.get("put") {
        let mut common_op = service::Operation::new();
        common_op.path = path.to_owned();
        common_op.method = service::operation::HttpMethodType::PUT.into();
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

    if let Some(op) = item.get("patch") {
        let mut common_op = service::Operation::new();
        common_op.path = path.to_owned();
        common_op.method = service::operation::HttpMethodType::PATCH.into();
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

    if let Some(op) = item.get("delete") {
        let mut common_op = service::Operation::new();
        common_op.path = path.to_owned();
        common_op.method = service::operation::HttpMethodType::DELETE.into();
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

    if let Some(op) = item.get("head") {
        let mut common_op = service::Operation::new();
        common_op.path = path.to_owned();
        common_op.method = service::operation::HttpMethodType::HEAD.into();
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

    if let Some(op) = item.get("options") {
        let mut common_op = service::Operation::new();
        common_op.path = path.to_owned();
        common_op.method = service::operation::HttpMethodType::OPTIONS.into();
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

    if let Some(op) = item.get("trace") {
        let mut common_op = service::Operation::new();
        common_op.path = path.to_owned();
        common_op.method = service::operation::HttpMethodType::TRACE.into();
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

    Ok(result)
}

///
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
        let mut common_param = service::Parameter::new();
        handle_parameter(&param, &mut common_param, root, fetcher, cache, schemas)?;
        sink.parameter.push(common_param);
    }

    if let Some(request_body) = source.get("requestBody") {
        let mut common_request_body = service::RequestBody::new();
        handle_request_body(
            request_body,
            &mut common_request_body,
            root,
            fetcher,
            cache,
            schemas,
        )?;
        sink.requestBody = protobuf::MessageField::some(common_request_body);
    }

    let responses: HashMap<String, serde_json::Value> = default_field(source, "responses")?;
    if !responses.is_empty() {
        let mut common_responses = service::ApiResponses::new();
        for (status, response) in &responses {
            let mut common_response = service::ApiResponse::new();
            handle_response(
                response,
                &mut common_response,
                root,
                fetcher,
                cache,
                schemas,
            )?;
            common_responses
                .apiResponses
                .insert(status.clone(), common_response);
        }
        sink.apiResponses = protobuf::MessageField::some(common_responses);
    }

    if let Some(pagination) = source.get("x-pagination") {
        let mut common_page = service::Pagination::new();
        handle_pagination(pagination, &mut common_page)?;
        sink.pagination = protobuf::MessageField::some(common_page);
    }

    Ok(())
}

///
fn handle_pagination(
    source: &serde_json::Value,
    sink: &mut service::Pagination,
) -> error::Result<()> {
    let mut results_path = service::pagination::ExtendedPath::new();
    results_path.set_jmesPath(default_field(source, "resultsPath")?);

    if let Some(page_offset) = source.get("pageOffset") {
        let mut common_page_offset = service::pagination::PageOffset::new();

        common_page_offset.pageOffsetParam = default_field(page_offset, "pageOffsetParam")?;
        common_page_offset.startPage =
            protobuf::MessageField::some(default_field::<i32>(page_offset, "startPage")?.into());
        common_page_offset.limitParam = default_field(page_offset, "limitParam")?;
        common_page_offset.maxLimit =
            protobuf::MessageField::some(default_field::<i32>(page_offset, "maxLimit")?.into());
        common_page_offset.resultsPath = protobuf::MessageField::some(results_path);

        sink.set_pageOffset(common_page_offset);
    } else if let Some(offset) = source.get("offset") {
        let mut common_offset = service::pagination::Offset::new();

        common_offset.offsetParam = default_field(offset, "offsetParam")?;
        common_offset.limitParam = default_field(offset, "limitParam")?;
        common_offset.maxLimit =
            protobuf::MessageField::some(default_field::<i32>(offset, "maxLimit")?.into());
        common_offset.resultsPath = protobuf::MessageField::some(results_path);

        sink.set_offset(common_offset);
    } else if let Some(next_url) = source.get("nextUrl") {
        let mut common_next_url = service::pagination::NextUrl::new();

        let mut next_url_path = service::pagination::ExtendedPath::new();
        next_url_path.set_jmesPath(default_field(next_url, "nextUrlPath")?);

        common_next_url.nextUrlPath = protobuf::MessageField::some(next_url_path);
        common_next_url.limitParam = default_field(next_url, "limitParam")?;
        common_next_url.maxLimit =
            protobuf::MessageField::some(default_field::<i32>(next_url, "maxLimit")?.into());
        common_next_url.resultsPath = protobuf::MessageField::some(results_path);

        sink.set_nextUrl(common_next_url);
    } else if let Some(cursor) = source.get("cursor") {
        let mut common_cursor = service::pagination::MultiCursor::new();

        let mut cursor_path = service::pagination::ExtendedPath::new();
        cursor_path.set_jmesPath(default_field(cursor, "cursorPath")?);

        common_cursor.cursorsPath = vec![cursor_path];
        common_cursor.cursorsParam = vec![default_field::<String>(cursor, "cursorParam")?];
        common_cursor.limitParam = default_field(cursor, "limitParam")?;
        common_cursor.maxLimit =
            protobuf::MessageField::some(default_field::<i32>(cursor, "maxLimit")?.into());
        common_cursor.resultsPath = protobuf::MessageField::some(results_path);

        sink.set_multiCursor(common_cursor);
    } else {
        let mut common_unpaginaged = service::pagination::Unpaginated::new();
        common_unpaginaged.resultsPath = protobuf::MessageField::some(results_path);
        sink.set_unpaginated(common_unpaginaged);
    }

    Ok(())
}

///
fn handle_parameter<R: io::Read>(
    source: &serde_json::Value,
    sink: &mut service::Parameter,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
) -> error::Result<()> {
    let reference = handle_reference(source, root, fetcher, cache, &mut HashSet::new())?;
    let source = reference.as_ref().map_or(source, |&(_, ref item)| item);

    let in_ = required_field::<String>(source, "in")?;
    let in_ = match in_.as_str() {
        "header" => service::parameter::InType::HEADER,
        "query" => service::parameter::InType::QUERY,
        "path" => service::parameter::InType::PATH,
        "cookie" => service::parameter::InType::COOKIE,
        _ => service::parameter::InType::IN_TYPE_NONE,
    };
    sink.in_ = in_.into();

    sink.name = required_field(source, "name")?;
    sink.required = default_field(source, "required")?;

    if let Some(description) = optional_field(source, "description")? {
        sink.description = description;
    }

    if let Some(schema) = source.get("schema") {
        let mut common_schema = service::Schema::new();
        handle_schema(schema, &mut common_schema, root, fetcher, cache, schemas)?;
        sink.schema = protobuf::MessageField::some(common_schema);
    }

    Ok(())
}

///
fn handle_request_body<R: io::Read>(
    source: &serde_json::Value,
    sink: &mut service::RequestBody,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
) -> error::Result<()> {
    let reference = handle_reference(source, root, fetcher, cache, &mut HashSet::new())?;
    let source = reference.as_ref().map_or(source, |&(_, ref item)| item);

    if let Some(description) = optional_field(source, "description")? {
        sink.description = description;
    }

    let content: HashMap<String, serde_json::Value> = default_field(source, "content")?;
    for (key, value) in &content {
        let mut common_media_type = service::MediaType::new();
        handle_media_type(value, &mut common_media_type, root, fetcher, cache, schemas)?;
        sink.content.insert(key.to_string(), common_media_type);
    }

    Ok(())
}

///
fn handle_response<R: io::Read>(
    source: &serde_json::Value,
    sink: &mut service::ApiResponse,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
) -> error::Result<()> {
    let reference = handle_reference(source, root, fetcher, cache, &mut HashSet::new())?;
    let source = reference.as_ref().map_or(source, |&(_, ref item)| item);

    let content: HashMap<String, serde_json::Value> = default_field(source, "content")?;
    for (key, value) in &content {
        let mut common_media_type = service::MediaType::new();
        handle_media_type(value, &mut common_media_type, root, fetcher, cache, schemas)?;
        sink.content.insert(key.to_string(), common_media_type);
    }

    // NOTE: Response Headers aren't included in the protobuf

    Ok(())
}

///
fn handle_media_type<R: io::Read>(
    source: &serde_json::Value,
    sink: &mut service::MediaType,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    schemas: &mut HashMap<String, service::Schema>,
) -> error::Result<()> {
    if let Some(schema) = source.get("schema") {
        let mut common_schema = service::Schema::new();
        handle_schema(schema, &mut common_schema, root, fetcher, cache, schemas)?;
        sink.schema = protobuf::MessageField::some(common_schema);
    }

    Ok(())
}

///
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
        sink.set_ref(key);
        return Ok(());
    }
    let reference = reference?;

    if let Some((key, source)) = reference {
        sink.set_ref(key.clone());

        if !schemas.contains_key(&key) {
            schemas.insert(key.clone(), service::Schema::new());
            let mut ref_type = service::Schema::new();
            handle_schema(&source, &mut ref_type, root, fetcher, cache, schemas)?;
            schemas.insert(key, ref_type);
        }
        return Ok(());
    }

    let type_ = optional_field::<String>(source, "type")?;

    if let Some(type_) = type_ {
        match type_.as_str() {
            "string" => sink.set_schemaObject(service::SchemaObject {
                type_: service::schema_object::SchemaType::STRING.into(),
                ..Default::default()
            }),
            "boolean" => sink.set_schemaObject(service::SchemaObject {
                type_: service::schema_object::SchemaType::BOOLEAN.into(),
                ..Default::default()
            }),
            "integer" => sink.set_schemaObject(service::SchemaObject {
                type_: service::schema_object::SchemaType::INTEGER.into(),
                ..Default::default()
            }),
            "number" => sink.set_schemaObject(service::SchemaObject {
                type_: service::schema_object::SchemaType::NUMBER.into(),
                ..Default::default()
            }),
            "array" => {
                let items = if let Some(items) = source.get("items") {
                    let mut common_items = service::Schema::new();
                    handle_schema(items, &mut common_items, root, fetcher, cache, schemas)?;
                    protobuf::MessageField::some(common_items)
                } else {
                    protobuf::MessageField::none()
                };

                sink.set_schemaObject(service::SchemaObject {
                    type_: service::schema_object::SchemaType::ARRAY.into(),
                    items,
                    ..Default::default()
                });
            }
            "object" => {
                let properties: HashMap<String, serde_json::Value> =
                    default_field(source, "properties")?;

                let properties: error::Result<HashMap<String, service::Schema>> = properties
                    .iter()
                    .map(|(key, value)| {
                        let mut common_property = service::Schema::new();
                        handle_schema(value, &mut common_property, root, fetcher, cache, schemas)?;
                        Ok((key.to_string(), common_property))
                    })
                    .collect();
                let properties = properties?;

                let required: Vec<String> = default_field(source, "required")?;
                sink.set_schemaObject(service::SchemaObject {
                    type_: service::schema_object::SchemaType::OBJECT.into(),
                    properties,
                    required,
                    ..Default::default()
                });
            }
            _ => {}
        }
    } else {
        let result = optional_field::<Vec<serde_json::Value>>(source, "oneOf")?;
        if let Some(result) = result {
            let schema: error::Result<Vec<service::Schema>> = result
                .iter()
                .map(|value| {
                    let mut common_schema = service::Schema::new();
                    handle_schema(value, &mut common_schema, root, fetcher, cache, schemas)?;
                    Ok(common_schema)
                })
                .collect();
            let schema = schema?;

            sink.set_oneOf(service::ComposedSchema {
                schema,
                ..Default::default()
            });
        }

        let result = optional_field::<Vec<serde_json::Value>>(source, "anyOf")?;
        if let Some(result) = result {
            let schema: error::Result<Vec<service::Schema>> = result
                .iter()
                .map(|value| {
                    let mut common_schema = service::Schema::new();
                    handle_schema(value, &mut common_schema, root, fetcher, cache, schemas)?;
                    Ok(common_schema)
                })
                .collect();
            let schema = schema?;

            sink.set_anyOf(service::ComposedSchema {
                schema,
                ..Default::default()
            });
        }

        let result = optional_field::<Vec<serde_json::Value>>(source, "allOf")?;
        if let Some(result) = result {
            let schema: error::Result<Vec<service::Schema>> = result
                .iter()
                .map(|value| {
                    let mut common_schema = service::Schema::new();
                    handle_schema(value, &mut common_schema, root, fetcher, cache, schemas)?;
                    Ok(common_schema)
                })
                .collect();
            let schema = schema?;

            sink.set_allOf(service::ComposedSchema {
                schema,
                ..Default::default()
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {
    #![allow(clippy::restriction, clippy::pedantic)]

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
        assert_eq!("https://example.com", root.basePath());

        Ok(())
    }

    #[test]
    fn test_basic_path() -> error::Result<()> {
        let doc = include_str!("stubs/basic_path.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let operation = root.operations.get("say_hello").unwrap();
        assert_eq!(
            service::operation::HttpMethodType::GET,
            operation.method.unwrap()
        );
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

        assert_eq!(service::parameter::InType::HEADER, param.in_.unwrap());
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

        assert_eq!(service::parameter::InType::HEADER, param.in_.unwrap());
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

        let req_body = path.requestBody.as_ref().unwrap();
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

        let req_body = path.requestBody.as_ref().unwrap();
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

        let ok_response = op.apiResponses.apiResponses.get("200").unwrap();
        assert_eq!(1, ok_response.content.len());

        Ok(())
    }

    #[test]
    fn test_path_item_responses_ref() -> error::Result<()> {
        let doc = include_str!("stubs/path_item_responses_ref.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();

        let ok_response = op.apiResponses.apiResponses.get("200").unwrap();
        assert_eq!(1, ok_response.content.len());

        Ok(())
    }

    #[test]
    fn test_path_item_pagination() -> error::Result<()> {
        let doc = include_str!("stubs/path_item_pagination.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();

        let page = &op.pagination;
        assert!(page.has_offset());

        let page = page.offset();

        assert_eq!(100, page.maxLimit.value);
        assert_eq!("$response.body#/", page.resultsPath.jmesPath());

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

        let schema = &param.schema;
        let schema = schema.schemaObject();
        assert_eq!(
            service::schema_object::SchemaType::STRING,
            schema.type_.unwrap()
        );

        Ok(())
    }

    #[test]
    fn test_array_schema() -> error::Result<()> {
        let doc = include_str!("stubs/array_schema.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();

        let param = &op.parameter[0];

        let schema = &param.schema;
        let schema = schema.schemaObject();

        assert_eq!(
            service::schema_object::SchemaType::ARRAY,
            schema.type_.unwrap()
        );

        let items = schema.items.schemaObject();
        assert_eq!(
            service::schema_object::SchemaType::STRING,
            items.type_.unwrap()
        );

        Ok(())
    }

    #[test]
    fn test_object_schema() -> error::Result<()> {
        let doc = include_str!("stubs/object_schema.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();

        let param = &op.parameter[0];

        let schema = &param.schema;
        let schema = schema.schemaObject();

        assert_eq!(
            service::schema_object::SchemaType::OBJECT,
            schema.type_.unwrap()
        );

        let props = &schema.properties;
        assert_eq!(
            service::schema_object::SchemaType::NUMBER,
            props.get("foo").unwrap().schemaObject().type_.unwrap()
        );

        let bar = props.get("bar").unwrap().schemaObject();
        assert_eq!(
            service::schema_object::SchemaType::OBJECT,
            bar.type_.unwrap()
        );

        let baz = bar.properties.get("baz").unwrap().schemaObject();
        assert_eq!(
            service::schema_object::SchemaType::STRING,
            baz.type_.unwrap()
        );

        Ok(())
    }

    #[test]
    fn test_oneof_schema() -> error::Result<()> {
        let doc = include_str!("stubs/oneof_schema.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;

        let op = root.operations.get("say_hello").unwrap();

        let param = &op.parameter[0];

        let schema = &param.schema;
        let schema = schema.oneOf();
        let schema = &schema.schema;

        assert_eq!(
            service::schema_object::SchemaType::STRING,
            schema[0].schemaObject().type_.unwrap()
        );
        assert_eq!(
            service::schema_object::SchemaType::NUMBER,
            schema[1].schemaObject().type_.unwrap()
        );

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
        assert_eq!(service::parameter::InType::HEADER, param.in_.unwrap());
        assert_eq!("Version", param.name);
        assert!(!param.required);

        let schema = param.schema.ref_();
        assert_eq!("#/components/schemas/Version", schema);

        let schema = root
            .schemas
            .get("#/components/schemas/Version")
            .unwrap()
            .schemaObject();
        assert_eq!(
            service::schema_object::SchemaType::STRING,
            schema.type_.unwrap()
        );

        Ok(())
    }

    #[test]
    fn test_schema_with_ref_cycle() -> error::Result<()> {
        let doc = include_str!("stubs/schema_with_ref_cycle.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;
        let op = root.operations.get("say_hello").unwrap();

        let param = &op.parameter[0];
        assert_eq!(service::parameter::InType::HEADER, param.in_.unwrap());
        assert_eq!("Version", param.name);
        assert!(!param.required);

        let schema = param.schema.ref_();
        assert_eq!("#/components/schemas/OtherVersion", schema);

        let schema = root
            .schemas
            .get("#/components/schemas/OtherVersion")
            .unwrap()
            .schemaObject();
        assert_eq!(
            service::schema_object::SchemaType::OBJECT,
            schema.type_.unwrap()
        );

        let schema = schema.properties.get("foo").unwrap().ref_();
        assert_eq!("#/components/schemas/OtherVersion", schema);

        Ok(())
    }

    #[test]
    fn test_oneof_schema_with_ref() -> error::Result<()> {
        let doc = include_str!("stubs/oneof_schema_with_ref.yaml");

        let fetcher = SimpleFetcher::new().with("main", doc);
        let root = handle(&fetcher, "main")?;
        let op = root.operations.get("say_hello").unwrap();

        let param = &op.parameter[0];
        assert_eq!(service::parameter::InType::HEADER, param.in_.unwrap());
        assert_eq!("Version", param.name);
        assert!(!param.required);

        let schemas = param.schema.oneOf();

        let schema = schemas.schema[0].schemaObject();
        assert_eq!(
            service::schema_object::SchemaType::STRING,
            schema.type_.unwrap()
        );

        let schema = schemas.schema[1].ref_();
        assert_eq!("#/components/schemas/Number", schema);

        let schema = root
            .schemas
            .get("#/components/schemas/Number")
            .unwrap()
            .schemaObject();
        assert_eq!(
            service::schema_object::SchemaType::NUMBER,
            schema.type_.unwrap()
        );

        Ok(())
    }
}
