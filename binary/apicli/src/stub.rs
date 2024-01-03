#![allow(clippy::separated_literal_suffix)]

//!

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail};
use common_data_structures::trie::Trie;
use core_entities::service::VersionedServiceTree;

///
pub fn get_input(
    service: &VersionedServiceTree,
    operation: &str,
    required: bool,
) -> anyhow::Result<serde_json::Value> {
    let service = service.v1();
    let manifest = &service.manifest.v2().value;

    let mut input_example = serde_json::Map::new();

    match manifest {
        &Some(core_entities::service::service_manifest_latest::Value::Swagger(_)) => {
            let operation = service
                .commonApi
                .operations
                .get(operation)
                .ok_or_else(|| anyhow!("Operation not found"))?;
            let types = &service.commonApi.schemas;

            for parameter in &operation.parameter {
                if required && !parameter.required {
                    continue;
                }
                let mut seen = HashMap::new();
                let mut path = vec![parameter.name.clone()];
                let default_value = schema_to_value(
                    &parameter.schema.value,
                    types,
                    &mut seen,
                    &mut path,
                    required,
                );
                input_example.insert(parameter.name.clone(), default_value);
            }

            if operation.requestBody.is_some() {
                let mut trie: Trie<core_entities::service::MediaType> = Trie::default();
                for (key, value) in &operation.requestBody.content {
                    trie.insert(key, value.clone());
                }

                if let Some(content) = trie.find("application/json") {
                    let mut seen = HashMap::new();
                    let mut path = vec!["$body".to_owned()];
                    input_example.insert(
                        "$body".to_owned(),
                        schema_to_value(
                            &content.schema.value,
                            types,
                            &mut seen,
                            &mut path,
                            required,
                        ),
                    );
                }
            }
        }
        &Some(core_entities::service::service_manifest_latest::Value::Action(ref manifest)) => {
            let operation = manifest
                .operations
                .iter()
                .find(|op| op.id == operation)
                .map(core_entities::service::action_service::ActionOperation::function)
                .ok_or_else(|| anyhow!("Operation not found"))?;

            for param in &operation.parameters {
                if required && !param.required {
                    continue;
                }
                let default_value = parameter_to_value(param.type_.enum_value_or_default());
                input_example.insert(param.name.clone(), default_value);
            }
        }
        &Some(core_entities::service::service_manifest_latest::Value::ApiWrapped(ref manifest)) => {
            for param in &manifest.inputs {
                let default_value = parameter_to_value(param.param.type_.enum_value_or_default());
                input_example.insert(param.param.name.clone(), default_value);
            }
        }
        &Some(core_entities::service::service_manifest_latest::Value::SimpleCode(_)) => {
            bail!("Unimplemented manifest type: SimpleCode")
        }
        &Some(core_entities::service::service_manifest_latest::Value::ScriptedAction(_)) => {
            bail!("Unimplemented manifest type: ScriptedAction")
        }
        _ => bail!("Unknown manifest type"),
    }

    Ok(serde_json::Value::Object(input_example))
}

///
pub fn get_output(
    service: &VersionedServiceTree,
    operation: &str,
) -> anyhow::Result<serde_json::Value> {
    let service = service.v1();
    let manifest = &service.manifest.v2().value;

    match manifest {
        &Some(core_entities::service::service_manifest_latest::Value::Swagger(_)) => {
            let operation = service
                .commonApi
                .operations
                .get(operation)
                .ok_or_else(|| anyhow!("Operation not found"))?;
            let types = &service.commonApi.schemas;

            if operation.apiResponses.is_some() {
                let mut status_codes: Trie<core_entities::service::ApiResponse> = Trie::default();
                for (key, value) in &operation.apiResponses.apiResponses {
                    status_codes.insert(key, value.clone());
                }

                if let Some(response) = status_codes.find("200") {
                    let mut trie: Trie<core_entities::service::MediaType> = Trie::default();
                    for (key, value) in &response.content {
                        trie.insert(key, value.clone());
                    }

                    if let Some(content) = trie.find("application/json") {
                        let mut seen = HashMap::new();
                        let mut path = vec![];

                        let output = schema_to_value(
                            &content.schema.value,
                            types,
                            &mut seen,
                            &mut path,
                            false,
                        );
                        Ok(output)
                    } else {
                        Ok(serde_json::Value::Object(serde_json::Map::new()))
                    }
                } else {
                    Ok(serde_json::Value::Object(serde_json::Map::new()))
                }
            } else {
                Ok(serde_json::Value::Object(serde_json::Map::new()))
            }
        }
        &Some(core_entities::service::service_manifest_latest::Value::Action(ref manifest)) => {
            let operation = manifest
                .operations
                .iter()
                .find(|op| op.id == operation)
                .map(core_entities::service::action_service::ActionOperation::function)
                .ok_or_else(|| anyhow!("Operation not found"))?;

            let mut output_examples = serde_json::Map::new();
            for param in &operation.outputs {
                let default_value = parameter_to_value(param.type_.unwrap());
                output_examples.insert(param.name.clone(), default_value);
            }

            Ok(serde_json::Value::Object(output_examples))
        }
        &Some(core_entities::service::service_manifest_latest::Value::ApiWrapped(ref manifest)) => {
            let mut output_examples = serde_json::Map::new();
            for param in &manifest.outputSelectors {
                // TODO: use JMES path to determine type
                let default_value = serde_json::Value::String("<UNKNOWN>".into());
                output_examples.insert(param.name.clone(), default_value);
            }

            Ok(serde_json::Value::Object(output_examples))
        }
        &Some(core_entities::service::service_manifest_latest::Value::SimpleCode(_)) => {
            bail!("Unimplemented manifest type: SimpleCode")
        }
        &Some(core_entities::service::service_manifest_latest::Value::ScriptedAction(_)) => {
            bail!("Unimplemented manifest type: ScriptedAction")
        }
        _ => bail!("Unknown manifest type"),
    }
}

///
pub fn parameter_to_value(
    param: core_entities::service::common_parameter::ParameterType,
) -> serde_json::Value {
    match param {
        core_entities::service::common_parameter::ParameterType::UNSET => {
            serde_json::Value::String("<UNKNOWN>".to_owned())
        }
        core_entities::service::common_parameter::ParameterType::STRING => {
            serde_json::Value::String(String::default())
        }
        core_entities::service::common_parameter::ParameterType::INTEGER
        | core_entities::service::common_parameter::ParameterType::NUMBER => {
            serde_json::Value::Number(serde_json::Number::from(0_i32))
        }
        core_entities::service::common_parameter::ParameterType::BOOLEAN => {
            serde_json::Value::Bool(false)
        }
        core_entities::service::common_parameter::ParameterType::OBJECT => {
            serde_json::Value::Object(serde_json::Map::new())
        }
        core_entities::service::common_parameter::ParameterType::ARRAY => {
            serde_json::Value::Array(vec![])
        }
    }
}

///
pub fn schema_to_value(
    schema: &Option<core_entities::service::schema::Value>,
    types: &HashMap<String, core_entities::service::Schema>,
    seen: &mut HashMap<String, String>,
    path: &mut Vec<String>,
    required: bool,
) -> serde_json::Value {
    match schema {
        &Some(core_entities::service::schema::Value::Ref(ref reference)) => {
            let schema = types.get(reference).cloned().and_then(|s| s.value);

            if seen.contains_key(reference) {
                return serde_json::Value::String(format!(
                    "$ref:{}",
                    seen.get(reference)
                        .cloned()
                        .unwrap_or_else(|| "Unknown Type".into())
                ));
            }

            seen.insert(reference.clone(), path.join("."));
            let schema = schema_to_value(&schema, types, seen, path, required);
            seen.remove(reference);
            schema
        }
        &Some(core_entities::service::schema::Value::SchemaObject(ref schema)) => {
            schema_object_to_value(schema, types, seen, path, required)
        }
        _ => serde_json::Value::Object(serde_json::Map::new()),
    }
}

///
pub fn schema_object_to_value(
    schema: &core_entities::service::SchemaObject,
    types: &HashMap<String, core_entities::service::Schema>,
    seen: &mut HashMap<String, String>,
    path: &mut Vec<String>,
    is_required: bool,
) -> serde_json::Value {
    match schema.type_.unwrap() {
        core_entities::service::schema_object::SchemaType::SCHEMA_TYPE_NONE => {
            serde_json::Value::String("<UNKNOWN>".to_owned())
        }
        core_entities::service::schema_object::SchemaType::STRING => {
            serde_json::Value::String(String::default())
        }
        core_entities::service::schema_object::SchemaType::NUMBER
        | core_entities::service::schema_object::SchemaType::INTEGER => {
            serde_json::Value::Number(serde_json::Number::from(0_i32))
        }
        core_entities::service::schema_object::SchemaType::BOOLEAN => {
            serde_json::Value::Bool(Default::default())
        }
        core_entities::service::schema_object::SchemaType::OBJECT => {
            let mut properties = serde_json::Map::new();
            let required: HashSet<String> = schema.required.iter().cloned().collect();

            for (key, value) in &schema.properties {
                if is_required && !required.contains(key) {
                    continue;
                }
                path.push(key.to_string());
                properties.insert(
                    key.to_string(),
                    schema_to_value(&value.value, types, seen, path, is_required),
                );
                path.pop();
            }

            serde_json::Value::Object(properties)
        }
        core_entities::service::schema_object::SchemaType::ARRAY => {
            path.push("0".to_owned());
            let items = vec![schema_to_value(
                &schema.items.value,
                types,
                seen,
                path,
                is_required,
            )];
            path.pop();
            serde_json::Value::Array(items)
        }
    }
}
