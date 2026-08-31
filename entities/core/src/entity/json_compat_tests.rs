//! Verifies the hand-written `entity` types parse and re-emit the exact
//! same JSON shape as the `protobuf`-generated `service` types did - the
//! actual regression net for `manifest.json`/`config.json` staying
//! readable across the migration off protobuf. Deleted once `service` is
//! removed.

use crate::{entity, service as old};

fn assert_same_json<T>(old_json: &str, new: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let new_json = serde_json::to_string(new).expect("new type serializes");

    let old_value: serde_json::Value = serde_json::from_str(old_json).expect("old JSON parses");
    let new_value: serde_json::Value = serde_json::from_str(&new_json).expect("new JSON parses");
    assert_eq!(
        old_value, new_value,
        "old proto JSON {old_json:?} and new serde JSON {new_json:?} must match"
    );

    let round_tripped: T = serde_json::from_str(old_json).expect("new type parses the old proto's JSON");
    assert_eq!(&round_tripped, new);
}

#[test]
fn swagger_manifest_round_trips_through_the_full_tree() {
    let mut old_op = old::Operation::new();
    old_op.path = "/widgets/{id}".into();
    old_op.method = protobuf::EnumOrUnknown::new(old::operation::HttpMethodType::GET);
    old_op.id = "getWidget".into();
    old_op.description = "Gets a widget".into();
    old_op.summary = "Get widget".into();

    let mut old_param = old::Parameter::new();
    old_param.name = "id".into();
    old_param.required = true;
    old_param.in_ = protobuf::EnumOrUnknown::new(old::parameter::InType::PATH);
    let mut id_schema = old::Schema::new();
    id_schema.set_ref("#/schemas/Id".into());
    old_param.schema = protobuf::MessageField::some(id_schema);
    old_op.parameter.push(old_param);

    let mut unpaginated = old::pagination::Unpaginated::new();
    let mut results_path = old::pagination::ExtendedPath::new();
    results_path.set_jmesPath("items".into());
    unpaginated.resultsPath = protobuf::MessageField::some(results_path);
    let mut err_flag = protobuf::well_known_types::wrappers::BoolValue::new();
    err_flag.value = true;
    unpaginated.errorOnPathNotFound = protobuf::MessageField::some(err_flag);
    let mut old_pagination = old::Pagination::new();
    old_pagination.set_unpaginated(unpaginated);
    old_op.pagination = protobuf::MessageField::some(old_pagination);

    let mut composed = old::ComposedSchema::new();
    let mut leaf = old::Schema::new();
    let mut obj = old::SchemaObject::new();
    obj.type_ = protobuf::EnumOrUnknown::new(old::schema_object::SchemaType::OBJECT);
    obj.name = "Widget".into();
    leaf.set_schemaObject(obj);
    composed.schema.push(leaf);
    let mut composed_schema = old::Schema::new();
    composed_schema.set_allOf(composed);

    let mut old_api = old::CommonApi::new();
    old_api.title = "Widgets API".into();
    old_api.set_basePath("https://api.example.com".into());
    old_api.operations.insert("getWidget".into(), old_op);
    old_api.schemas.insert("Widget".into(), composed_schema);

    let mut old_oauth = old::service_manifest_latest::OAuthConfig::new();
    old_oauth.authUri = "https://auth.example.com/authorize".into();
    old_oauth.parameterLocation = protobuf::EnumOrUnknown::new(
        old::service_manifest_latest::oauth_config::ParameterLocation::BODY,
    );

    let mut old_service_auth = old::swagger_service::ServiceAuth::new();
    old_service_auth.type_ =
        protobuf::EnumOrUnknown::new(old::swagger_service::service_auth::Type::OAUTH);
    old_service_auth.set_oauthConfig(old_oauth);

    let mut old_swagger = old::SwaggerService::new();
    old_swagger.auth = protobuf::MessageField::some(old_service_auth);
    old_swagger.source = "./".into();
    old_swagger.url = "https://api.example.com".into();
    old_swagger
        .serverVariables
        .insert("region".into(), "us-east-1".into());

    let mut old_latest = old::ServiceManifestLatest::new();
    old_latest.description = "A widgets connector".into();
    old_latest.displayName = "Widgets".into();
    old_latest.set_swagger(old_swagger);

    let mut old_manifest = old::ServiceManifest::new();
    old_manifest.set_v2(old_latest);

    let mut old_tree = old::VersionedServiceTree::new();
    old_tree.mut_v1().manifest = protobuf::MessageField::some(old_manifest);
    old_tree.mut_v1().commonApi = protobuf::MessageField::some(old_api);

    let old_json = protobuf_json_mapping::print_to_string(&old_tree).unwrap();

    let new_tree = entity::VersionedServiceTree {
        version: Some(entity::versioned_service_tree::Version::V1(
            entity::versioned_service_tree::V1 {
                manifest: Some(entity::ServiceManifest {
                    value: Some(entity::service_manifest::Value::V2(
                        entity::ServiceManifestLatest {
                            description: "A widgets connector".into(),
                            display_name: "Widgets".into(),
                            value: Some(entity::service_manifest_latest::Value::Swagger(
                                Box::new(entity::SwaggerService {
                                    auth: Some(entity::swagger_service::ServiceAuth {
                                        r#type: entity::swagger_service::service_auth::Type::Oauth,
                                        oauth_config: Some(entity::service_manifest_latest::OAuthConfig {
                                            auth_uri: "https://auth.example.com/authorize".into(),
                                            parameter_location:
                                                entity::service_manifest_latest::oauth_config::ParameterLocation::Body,
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    }),
                                    source: "./".into(),
                                    url: "https://api.example.com".into(),
                                    server_variables: [("region".to_owned(), "us-east-1".to_owned())]
                                        .into_iter()
                                        .collect(),
                                }),
                            )),
                        },
                    )),
                }),
                common_api: Some(entity::CommonApi {
                    base_path: Some("https://api.example.com".into()),
                    title: "Widgets API".into(),
                    operations: [(
                        "getWidget".to_owned(),
                        entity::Operation {
                            path: "/widgets/{id}".into(),
                            method: entity::operation::HttpMethodType::Get,
                            id: "getWidget".into(),
                            description: "Gets a widget".into(),
                            summary: "Get widget".into(),
                            parameter: vec![entity::Parameter {
                                name: "id".into(),
                                required: true,
                                r#in: entity::parameter::InType::Path,
                                schema: Some(entity::Schema::Ref("#/schemas/Id".into())),
                                ..Default::default()
                            }],
                            pagination: Some(entity::Pagination::Unpaginated(
                                entity::pagination::Unpaginated {
                                    results_path: Some(entity::pagination::ExtendedPath::JmesPath(
                                        "items".into(),
                                    )),
                                    error_on_path_not_found: Some(true),
                                },
                            )),
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                    schemas: [(
                        "Widget".to_owned(),
                        entity::Schema::AllOf(entity::ComposedSchema {
                            schema: vec![entity::Schema::SchemaObject(entity::SchemaObject {
                                r#type: entity::schema_object::SchemaType::Object,
                                name: "Widget".into(),
                                ..Default::default()
                            })],
                        }),
                    )]
                    .into_iter()
                    .collect(),
                    ..Default::default()
                }),
                resources: vec![],
            },
        )),
    };

    assert_same_json(&old_json, &new_tree);
}

#[test]
fn action_manifest_round_trips() {
    let mut old_func = old::FunctionOperation::new();
    old_func.lang = "js".into();
    old_func.set_js("index.js".into());
    let mut old_op = old::action_service::ActionOperation::new();
    old_op.id = "run".into();
    old_op.set_function(old_func);
    let mut old_action = old::ActionService::new();
    old_action.operations.push(old_op);
    old_action.source = "./scripts".into();

    let mut old_latest = old::ServiceManifestLatest::new();
    old_latest.set_action(old_action);
    let mut old_manifest = old::ServiceManifest::new();
    old_manifest.set_v2(old_latest);
    let old_json = protobuf_json_mapping::print_to_string(&old_manifest).unwrap();

    let new_manifest = entity::ServiceManifest {
        value: Some(entity::service_manifest::Value::V2(
            entity::ServiceManifestLatest {
                value: Some(entity::service_manifest_latest::Value::Action(
                    entity::ActionService {
                        operations: vec![entity::action_service::ActionOperation {
                            id: "run".into(),
                            function: Some(entity::FunctionOperation {
                                lang: "js".into(),
                                js: Some("index.js".into()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }],
                        source: "./scripts".into(),
                    },
                )),
                ..Default::default()
            },
        )),
    };

    assert_same_json(&old_json, &new_manifest);
}

#[test]
fn api_wrapped_manifest_round_trips() {
    let mut old_param = old::apiwrapped_service::Parameter::new();
    old_param.apiParamName = "widgetId".into();
    let mut old_op_param = old::OperationParameter::new();
    old_op_param.name = "id".into();
    old_op_param.type_ =
        protobuf::EnumOrUnknown::new(old::common_parameter::ParameterType::STRING);
    old_param.param = protobuf::MessageField::some(old_op_param);

    let mut old_selector = old::OutputSelector::new();
    old_selector.name = "id".into();
    old_selector.jmesPathSelector = "id".into();

    let mut old_wrapped = old::APIWrappedService::new();
    old_wrapped.connectorId = "widgets".into();
    old_wrapped.connectorOperation = "getWidget".into();
    old_wrapped.inputs.push(old_param);
    old_wrapped.outputSelectors.push(old_selector);

    let old_json = protobuf_json_mapping::print_to_string(&old_wrapped).unwrap();

    let new_wrapped = entity::APIWrappedService {
        connector_id: "widgets".into(),
        connector_operation: "getWidget".into(),
        inputs: vec![entity::apiwrapped_service::Parameter {
            api_param_name: "widgetId".into(),
            param: Some(entity::OperationParameter {
                name: "id".into(),
                r#type: entity::common_parameter::ParameterType::String,
                ..Default::default()
            }),
        }],
        output_selectors: vec![entity::OutputSelector {
            name: "id".into(),
            jmes_path_selector: "id".into(),
        }],
    };

    assert_same_json(&old_json, &new_wrapped);
}

#[test]
fn simple_code_manifest_round_trips() {
    let mut old_code = old::CodeResource::new();
    old_code.set_codeString("return 1".into());
    old_code.language = protobuf::EnumOrUnknown::new(old::code_resource::Language::PYTHON);
    let mut old_simple = old::SimpleCodeService::new();
    old_simple.code = protobuf::MessageField::some(old_code);

    let old_json = protobuf_json_mapping::print_to_string(&old_simple).unwrap();

    let new_simple = entity::SimpleCodeService {
        code: Some(entity::CodeResource {
            language: entity::code_resource::Language::Python,
            value: Some(entity::code_resource::Value::CodeString("return 1".into())),
        }),
        ..Default::default()
    };

    assert_same_json(&old_json, &new_simple);
}

#[test]
fn scripted_action_manifest_round_trips() {
    let mut old_input = old::OperationParameter::new();
    old_input.name = "x".into();
    let mut old_scripted = old::ScriptedAction::new();
    old_scripted.inputs.push(old_input);

    let old_json = protobuf_json_mapping::print_to_string(&old_scripted).unwrap();

    let new_scripted = entity::ScriptedAction {
        inputs: vec![entity::OperationParameter {
            name: "x".into(),
            ..Default::default()
        }],
    };

    assert_same_json(&old_json, &new_scripted);
}

#[test]
fn workflow_manifest_round_trips_with_u64_as_string() {
    let mut old_workflow = old::WorkflowService::new();
    old_workflow.set_codeString("return 42".into());
    old_workflow.timeoutSeconds = 30;
    old_workflow.memoryLimitBytes = 268_435_456;

    let old_json = protobuf_json_mapping::print_to_string(&old_workflow).unwrap();
    assert!(
        old_json.contains("\"268435456\""),
        "memoryLimitBytes should be a JSON string in the old proto mapping: {old_json}"
    );

    let new_workflow = entity::WorkflowService {
        source: Some(entity::workflow_service::Source::CodeString("return 42".into())),
        timeout_seconds: 30,
        memory_limit_bytes: 268_435_456,
    };

    assert_same_json(&old_json, &new_workflow);
}

#[test]
fn empty_manifest_omits_every_default_valued_field() {
    let old_tree = old::VersionedServiceTree::new();
    let old_json = protobuf_json_mapping::print_to_string(&old_tree).unwrap();
    assert_eq!(old_json, "{}", "an unset VersionedServiceTree should print as {{}}");

    let new_tree = entity::VersionedServiceTree::default();
    assert_same_json(&old_json, &new_tree);
}
