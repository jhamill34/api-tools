//! Generates a sample JSON input/output payload for an operation, used by
//! the `InputStub`/`OutputStub` CLI commands.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail};
use common_data_structures::trie::Trie;
use core_entities::entity::{
    common_parameter::ParameterType, schema_object::SchemaType, service_manifest_latest,
    ApiResponse, CommonApi, MediaType, Schema, SchemaObject, SchemaValue, VersionedServiceTree,
};

/// Builds a sample input payload for `operation`, driven by the service's
/// manifest type: parameter/schema defaults for an `OpenAPI` (`Swagger`)
/// operation's parameters and JSON request body, or per-parameter defaults
/// for an `Action`/`ApiWrapped` manifest. Errors on `SimpleCode`/
/// `ScriptedAction` manifests, which aren't yet supported.
pub fn get_input(
    service: &VersionedServiceTree,
    operation: &str,
    required: bool,
) -> anyhow::Result<serde_json::Value> {
    let v1 = service.v1();
    let manifest = v1.manifest_latest();

    let mut input_example = serde_json::Map::new();

    match &manifest.value {
        Some(service_manifest_latest::Value::Swagger(_)) => {
            let empty_api = CommonApi::default();
            let api = v1.common_api.as_ref().unwrap_or(&empty_api);

            let operation = api
                .operations
                .get(operation)
                .ok_or_else(|| anyhow!("Operation not found"))?;
            let types = &api.schemas;

            for parameter in &operation.parameter {
                if required && !parameter.required {
                    continue;
                }
                let mut seen = HashMap::new();
                let mut path = vec![parameter.name.clone()];
                let default_value = schema_to_value(
                    parameter.schema.as_ref(),
                    types,
                    &mut seen,
                    &mut path,
                    required,
                );
                input_example.insert(parameter.name.clone(), default_value);
            }

            if let Some(request_body) = &operation.request_body {
                let mut trie: Trie<MediaType> = Trie::default();
                for (key, value) in &request_body.content {
                    trie.insert(key, value.clone());
                }

                if let Some(content) = trie.find("application/json") {
                    let mut seen = HashMap::new();
                    let mut path = vec!["$body".to_owned()];
                    input_example.insert(
                        "$body".to_owned(),
                        schema_to_value(content.schema.as_ref(), types, &mut seen, &mut path, required),
                    );
                }
            }
        }
        Some(service_manifest_latest::Value::Action(manifest)) => {
            let operation = manifest
                .operations
                .iter()
                .find(|op| op.id == operation)
                .and_then(|op| op.function.as_ref())
                .ok_or_else(|| anyhow!("Operation not found"))?;

            for param in &operation.parameters {
                if required && !param.required {
                    continue;
                }
                let default_value = parameter_to_value(param.r#type);
                input_example.insert(param.name.clone(), default_value);
            }
        }
        Some(service_manifest_latest::Value::ApiWrapped(manifest)) => {
            for param in &manifest.inputs {
                let (param_type, param_name) = param.param.as_ref().map_or_else(
                    || (ParameterType::Unset, String::new()),
                    |p| (p.r#type, p.name.clone()),
                );
                let default_value = parameter_to_value(param_type);
                input_example.insert(param_name, default_value);
            }
        }
        Some(service_manifest_latest::Value::SimpleCode(_)) => {
            bail!("Unimplemented manifest type: SimpleCode")
        }
        Some(service_manifest_latest::Value::ScriptedAction(_)) => {
            bail!("Unimplemented manifest type: ScriptedAction")
        }
        Some(service_manifest_latest::Value::Workflow(_)) | None => {
            bail!("Unknown manifest type")
        }
    }

    Ok(serde_json::Value::Object(input_example))
}

/// Builds a sample output payload for `operation`, mirroring
/// [`get_input`]'s per-manifest-type handling: the `OpenAPI` `200` response's
/// JSON schema for a `Swagger` operation, per-parameter defaults for an
/// `Action` manifest's outputs, or `"<UNKNOWN>"` placeholders for an
/// `ApiWrapped` manifest's output selectors (their real type isn't known
/// without evaluating their `JMESPath` expression). Errors on
/// `SimpleCode`/`ScriptedAction` manifests, which aren't yet supported.
pub fn get_output(
    service: &VersionedServiceTree,
    operation: &str,
) -> anyhow::Result<serde_json::Value> {
    let v1 = service.v1();
    let manifest = v1.manifest_latest();

    match &manifest.value {
        Some(service_manifest_latest::Value::Swagger(_)) => {
            let empty_api = CommonApi::default();
            let api = v1.common_api.as_ref().unwrap_or(&empty_api);

            let operation = api
                .operations
                .get(operation)
                .ok_or_else(|| anyhow!("Operation not found"))?;
            let types = &api.schemas;

            let Some(api_responses) = &operation.api_responses else {
                return Ok(serde_json::Value::Object(serde_json::Map::new()));
            };

            let mut status_codes: Trie<ApiResponse> = Trie::default();
            for (key, value) in &api_responses.api_responses {
                status_codes.insert(key, value.clone());
            }

            let Some(response) = status_codes.find("200") else {
                return Ok(serde_json::Value::Object(serde_json::Map::new()));
            };

            let mut trie: Trie<MediaType> = Trie::default();
            for (key, value) in &response.content {
                trie.insert(key, value.clone());
            }

            let Some(content) = trie.find("application/json") else {
                return Ok(serde_json::Value::Object(serde_json::Map::new()));
            };

            let mut seen = HashMap::new();
            let mut path = vec![];

            let output = schema_to_value(content.schema.as_ref(), types, &mut seen, &mut path, false);
            Ok(output)
        }
        Some(service_manifest_latest::Value::Action(manifest)) => {
            let operation = manifest
                .operations
                .iter()
                .find(|op| op.id == operation)
                .and_then(|op| op.function.as_ref())
                .ok_or_else(|| anyhow!("Operation not found"))?;

            let mut output_examples = serde_json::Map::new();
            for param in &operation.outputs {
                let default_value = parameter_to_value(param.r#type);
                output_examples.insert(param.name.clone(), default_value);
            }

            Ok(serde_json::Value::Object(output_examples))
        }
        Some(service_manifest_latest::Value::ApiWrapped(manifest)) => {
            let mut output_examples = serde_json::Map::new();
            for param in &manifest.output_selectors {
                // TODO: use JMES path to determine type
                let default_value = serde_json::Value::String("<UNKNOWN>".into());
                output_examples.insert(param.name.clone(), default_value);
            }

            Ok(serde_json::Value::Object(output_examples))
        }
        Some(service_manifest_latest::Value::SimpleCode(_)) => {
            bail!("Unimplemented manifest type: SimpleCode")
        }
        Some(service_manifest_latest::Value::ScriptedAction(_)) => {
            bail!("Unimplemented manifest type: ScriptedAction")
        }
        Some(service_manifest_latest::Value::Workflow(_)) | None => {
            bail!("Unknown manifest type")
        }
    }
}

/// Produces a zero-value/placeholder JSON value matching `param`'s type.
/// An unrecognized type produces `"<UNKNOWN>"` rather than panicking.
pub fn parameter_to_value(param: ParameterType) -> serde_json::Value {
    match param {
        ParameterType::Unset => serde_json::Value::String("<UNKNOWN>".to_owned()),
        ParameterType::String => serde_json::Value::String(String::default()),
        ParameterType::Integer | ParameterType::Number => {
            serde_json::Value::Number(serde_json::Number::from(0_i32))
        }
        ParameterType::Boolean => serde_json::Value::Bool(false),
        ParameterType::Object => serde_json::Value::Object(serde_json::Map::new()),
        ParameterType::Array => serde_json::Value::Array(vec![]),
    }
}

/// Produces a sample value for `schema`: resolves a `$ref` against
/// `types` (tracking `seen` references to print `$ref:{path}` instead of
/// recursing forever on a cycle) or delegates to
/// [`schema_object_to_value`] for an inline schema object. `path` tracks
/// the current field path, for cycle-reference display. A `None`/empty
/// `{}` schema (see [`Schema`]'s doc comment) produces an empty object.
pub fn schema_to_value(
    schema: Option<&Schema>,
    types: &HashMap<String, Schema>,
    seen: &mut HashMap<String, String>,
    path: &mut Vec<String>,
    required: bool,
) -> serde_json::Value {
    match schema.and_then(|s| s.value.as_ref()) {
        Some(SchemaValue::Ref(reference)) => {
            if seen.contains_key(reference) {
                return serde_json::Value::String(format!(
                    "$ref:{}",
                    seen.get(reference)
                        .cloned()
                        .unwrap_or_else(|| "Unknown Type".into())
                ));
            }

            seen.insert(reference.clone(), path.join("."));
            let value = schema_to_value(types.get(reference), types, seen, path, required);
            seen.remove(reference);
            value
        }
        Some(SchemaValue::SchemaObject(schema)) => {
            schema_object_to_value(schema, types, seen, path, required)
        }
        Some(SchemaValue::AllOf(_) | SchemaValue::AnyOf(_) | SchemaValue::OneOf(_)) | None => {
            serde_json::Value::Object(serde_json::Map::new())
        }
    }
}

/// Produces a zero-value sample matching `schema`'s type: a default
/// scalar, an object built by recursing into (optionally
/// `required`-filtered) properties, or a one-element array built by
/// recursing into the item schema. An unrecognized/unset type produces
/// `"<UNKNOWN>"` rather than panicking.
pub fn schema_object_to_value(
    schema: &SchemaObject,
    types: &HashMap<String, Schema>,
    seen: &mut HashMap<String, String>,
    path: &mut Vec<String>,
    is_required: bool,
) -> serde_json::Value {
    match schema.r#type {
        SchemaType::None => serde_json::Value::String("<UNKNOWN>".to_owned()),
        SchemaType::String => serde_json::Value::String(String::default()),
        SchemaType::Number | SchemaType::Integer => {
            serde_json::Value::Number(serde_json::Number::from(0_i32))
        }
        SchemaType::Boolean => serde_json::Value::Bool(bool::default()),
        SchemaType::Object => {
            let mut properties = serde_json::Map::new();
            let required: HashSet<String> = schema.required.iter().cloned().collect();

            for (key, value) in &schema.properties {
                if is_required && !required.contains(key) {
                    continue;
                }
                path.push(key.clone());
                properties.insert(
                    key.clone(),
                    schema_to_value(Some(value), types, seen, path, is_required),
                );
                path.pop();
            }

            serde_json::Value::Object(properties)
        }
        SchemaType::Array => {
            path.push("0".to_owned());
            let items = vec![schema_to_value(
                schema.items.as_deref(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_to_value_does_not_panic_on_an_unset_parameter_type() {
        let value = parameter_to_value(ParameterType::Unset);

        assert_eq!(value, serde_json::Value::String("<UNKNOWN>".to_owned()));
    }

    #[test]
    fn schema_object_to_value_does_not_panic_on_an_unset_schema_type() {
        let schema = SchemaObject {
            r#type: SchemaType::None,
            ..Default::default()
        };
        let types = HashMap::new();
        let mut seen = HashMap::new();
        let mut path = vec![];

        let value = schema_object_to_value(&schema, &types, &mut seen, &mut path, false);

        assert_eq!(value, serde_json::Value::String("<UNKNOWN>".to_owned()));
    }
}
