//! Generates a flat listing of an operation's input/output field paths,
//! used by the `InputPaths`/`OutputPaths` CLI commands.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail};
use common_data_structures::trie::Trie;
use core_entities::service::VersionedServiceTree;

/// Lists `operation`'s input fields as one [`ParameterPathItem`] per field,
/// driven by the service's manifest type: schema-derived paths for an
/// `OpenAPI` (`Swagger`) operation's parameters and JSON request body, or a
/// flat list of parameter names for an `Action` manifest. Errors on
/// `ApiWrapped`/`SimpleCode`/`ScriptedAction` manifests, which aren't yet
/// supported.
pub fn get_input_paths(
    service: &VersionedServiceTree,
    operation: &str,
    required: bool,
) -> anyhow::Result<Vec<ParameterPathItem>> {
    let service = service.v1();
    let manifest = &service.manifest.v2().value;

    let mut input_paths = Vec::new();

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
                let mut prefix = vec![];
                populate_schema_list(
                    &mut input_paths,
                    &parameter.schema.value,
                    types,
                    &mut seen,
                    &mut path,
                    required,
                    &mut prefix,
                );
            }

            if operation.requestBody.is_some() {
                let mut trie: Trie<core_entities::service::MediaType> = Trie::default();
                for (key, value) in &operation.requestBody.content {
                    trie.insert(key, value.clone());
                }

                if let Some(content) = trie.find("application/json") {
                    let mut seen = HashMap::new();
                    let mut path = vec!["$body".to_owned()];
                    let mut prefix = vec![];
                    populate_schema_list(
                        &mut input_paths,
                        &content.schema.value,
                        types,
                        &mut seen,
                        &mut path,
                        required,
                        &mut prefix,
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
                populate_parameter_list(
                    &mut input_paths,
                    param.type_,
                    &param.name,
                    &param.description,
                );
            }
        }
        &Some(core_entities::service::service_manifest_latest::Value::ApiWrapped(_)) => {
            bail!("Unimplemented manifest type: ApiWrapped")
        }
        &Some(core_entities::service::service_manifest_latest::Value::SimpleCode(_)) => {
            bail!("Unimplemented manifest type: SimpleCode")
        }
        &Some(core_entities::service::service_manifest_latest::Value::ScriptedAction(_)) => {
            bail!("Unimplemented manifest type: ScriptedAction")
        }
        _ => bail!("Unknown manifest type"),
    }

    Ok(input_paths)
}

/// Lists `operation`'s output fields as one [`ParameterPathItem`] per
/// field, mirroring [`get_input_paths`]'s per-manifest-type handling: the
/// `OpenAPI` `200` response's JSON schema for a `Swagger` operation, or a
/// flat list of output names for an `Action` manifest. Errors on
/// `ApiWrapped`/`SimpleCode`/`ScriptedAction` manifests, which aren't yet
/// supported.
pub fn get_output_paths(
    service: &VersionedServiceTree,
    operation: &str,
) -> anyhow::Result<Vec<ParameterPathItem>> {
    let service = service.v1();
    let manifest = &service.manifest.v2().value;

    let mut output_paths = Vec::new();

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
                        let mut prefix = vec![];

                        populate_schema_list(
                            &mut output_paths,
                            &content.schema.value,
                            types,
                            &mut seen,
                            &mut path,
                            false,
                            &mut prefix,
                        );
                    }
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

            for param in &operation.outputs {
                populate_parameter_list(
                    &mut output_paths,
                    param.type_,
                    &param.name,
                    &param.description,
                );
            }
        }
        &Some(core_entities::service::service_manifest_latest::Value::ApiWrapped(_)) => {
            bail!("Unimplemented manifest type: ApiWrapped")
        }
        &Some(core_entities::service::service_manifest_latest::Value::SimpleCode(_)) => {
            bail!("Unimplemented manifest type: SimpleCode")
        }
        &Some(core_entities::service::service_manifest_latest::Value::ScriptedAction(_)) => {
            bail!("Unimplemented manifest type: ScriptedAction")
        }
        _ => bail!("Unknown manifest type"),
    }

    Ok(output_paths)
}

/// One line of an input/output field listing: a field's path, type,
/// composition context (e.g. which `oneOf`/`anyOf` branch it came from),
/// and description.
pub struct ParameterPathItem {
    /// The field's dotted/indexed path (e.g. `foo.bar[0]`), or a `$ref:`
    /// marker if the path revisits an already-seen reference.
    pub path: String,

    /// The field's type name (`STRING`, `OBJECT`, ...), or `UNKNOWN` for
    /// an unrecognized/unset type.
    pub type_: String,

    /// Which `oneOf`/`anyOf` branch this field belongs to, if any.
    pub context: Option<String>,

    /// The field's description, if any.
    pub description: String,
}

impl ParameterPathItem {
    /// Creates a [`ParameterPathItem`] from its parts.
    pub fn new(path: String, type_: String, context: Option<String>, description: String) -> Self {
        Self {
            path,
            type_,
            context,
            description,
        }
    }
}

/// Appends a single [`ParameterPathItem`] for `name` with `param`'s type
/// name (`"UNKNOWN"` for the unset/default variant, rather than
/// panicking).
pub fn populate_parameter_list(
    list: &mut Vec<ParameterPathItem>,
    param: protobuf::EnumOrUnknown<core_entities::service::common_parameter::ParameterType>,
    name: &str,
    description: &str,
) {
    match param.enum_value_or_default() {
        core_entities::service::common_parameter::ParameterType::UNSET => {
            list.push(ParameterPathItem::new(
                name.to_owned(),
                "UNKNOWN".to_owned(),
                None,
                description.to_owned(),
            ));
        }
        core_entities::service::common_parameter::ParameterType::STRING => {
            list.push(ParameterPathItem::new(
                name.to_owned(),
                "STRING".to_owned(),
                None,
                description.to_owned(),
            ));
        }
        core_entities::service::common_parameter::ParameterType::INTEGER => {
            list.push(ParameterPathItem::new(
                name.to_owned(),
                "INTEGER".to_owned(),
                None,
                description.to_owned(),
            ));
        }
        core_entities::service::common_parameter::ParameterType::NUMBER => {
            list.push(ParameterPathItem::new(
                name.to_owned(),
                "NUMBER".to_owned(),
                None,
                description.to_owned(),
            ));
        }
        core_entities::service::common_parameter::ParameterType::BOOLEAN => {
            list.push(ParameterPathItem::new(
                name.to_owned(),
                "BOOLEAN".to_owned(),
                None,
                description.to_owned(),
            ));
        }
        core_entities::service::common_parameter::ParameterType::OBJECT => {
            list.push(ParameterPathItem::new(
                name.to_owned(),
                "OBJECT".to_owned(),
                None,
                description.to_owned(),
            ));
        }
        core_entities::service::common_parameter::ParameterType::ARRAY => {
            list.push(ParameterPathItem::new(
                name.to_owned(),
                "ARRAY".to_owned(),
                None,
                description.to_owned(),
            ));
        }
    }
}

/// Appends [`ParameterPathItem`]s for `schema` into `list`: resolves a
/// `$ref` against `types` (tracking `seen` references to emit a `$ref:`
/// marker instead of recursing forever on a cycle), delegates to
/// [`populate_schema_object_list`] for an inline schema object, or
/// recurses into each branch of an `allOf`/`oneOf`/`anyOf` composition
/// (tagging `oneOf`/`anyOf` branches in `prefix`).
pub fn populate_schema_list(
    list: &mut Vec<ParameterPathItem>,
    schema: &Option<core_entities::service::schema::Value>,
    types: &HashMap<String, core_entities::service::Schema>,
    seen: &mut HashMap<String, String>,
    path: &mut Vec<String>,
    is_required: bool,
    prefix: &mut Vec<String>,
) {
    match schema {
        &Some(core_entities::service::schema::Value::Ref(ref reference)) => {
            let schema = types.get(reference).cloned().and_then(|s| s.value);

            if seen.contains_key(reference) {
                let ref_type = format!(
                    "$ref:{}",
                    seen.get(reference)
                        .cloned()
                        .unwrap_or_else(|| "Unknown Type".to_owned())
                );
                list.push(ParameterPathItem::new(
                    path.join(""),
                    ref_type,
                    Some(prefix.join("|")),
                    String::new(),
                ));
                return;
            }

            seen.insert(reference.clone(), path.join(""));
            populate_schema_list(list, &schema, types, seen, path, is_required, prefix);
            seen.remove(reference);
        }
        &Some(core_entities::service::schema::Value::SchemaObject(ref schema)) => {
            populate_schema_object_list(list, schema, types, seen, path, is_required, prefix);
        }
        &Some(core_entities::service::schema::Value::AllOf(ref all_of)) => {
            for schema in &all_of.schema {
                populate_schema_list(list, &schema.value, types, seen, path, is_required, prefix);
            }
        }
        &Some(core_entities::service::schema::Value::OneOf(ref one_of)) => {
            for (idx, schema) in one_of.schema.iter().enumerate() {
                prefix.push(format!("one:{idx}"));
                populate_schema_list(list, &schema.value, types, seen, path, is_required, prefix);
                prefix.pop();
            }
        }
        &Some(core_entities::service::schema::Value::AnyOf(ref any_of)) => {
            for (idx, schema) in any_of.schema.iter().enumerate() {
                prefix.push(format!("any:{idx}"));
                populate_schema_list(list, &schema.value, types, seen, path, is_required, prefix);
                prefix.pop();
            }
        }
        _ => {}
    }
}

/// Appends a [`ParameterPathItem`] for `schema` itself, then — for an
/// object or array — recurses into its (optionally `is_required`-filtered)
/// properties or item schema, extending `path` as it goes. An
/// unrecognized/unset type is listed as `"UNKNOWN"` rather than panicking.
pub fn populate_schema_object_list(
    list: &mut Vec<ParameterPathItem>,
    schema: &core_entities::service::SchemaObject,
    types: &HashMap<String, core_entities::service::Schema>,
    seen: &mut HashMap<String, String>,
    path: &mut Vec<String>,
    is_required: bool,
    prefix: &mut Vec<String>,
) {
    let path_str = path.join("");
    let prefix_str = prefix.join("|");
    match schema.type_.enum_value_or_default() {
        core_entities::service::schema_object::SchemaType::SCHEMA_TYPE_NONE => {
            list.push(ParameterPathItem::new(
                path_str,
                "UNKNOWN".to_owned(),
                Some(prefix_str),
                schema.description.clone(),
            ));
        }
        core_entities::service::schema_object::SchemaType::STRING => {
            list.push(ParameterPathItem::new(
                path_str,
                "STRING".to_owned(),
                Some(prefix_str),
                schema.description.clone(),
            ));
        }
        core_entities::service::schema_object::SchemaType::NUMBER => {
            list.push(ParameterPathItem::new(
                path_str,
                "NUMBER".to_owned(),
                Some(prefix_str),
                schema.description.clone(),
            ));
        }
        core_entities::service::schema_object::SchemaType::INTEGER => {
            list.push(ParameterPathItem::new(
                path_str,
                "INTEGER".to_owned(),
                Some(prefix_str),
                schema.description.clone(),
            ));
        }
        core_entities::service::schema_object::SchemaType::BOOLEAN => {
            list.push(ParameterPathItem::new(
                path_str,
                "BOOLEAN".to_owned(),
                Some(prefix_str),
                schema.description.clone(),
            ));
        }
        core_entities::service::schema_object::SchemaType::OBJECT => {
            list.push(ParameterPathItem::new(
                path_str,
                "OBJECT".to_owned(),
                Some(prefix_str),
                schema.description.clone(),
            ));
            let required: HashSet<String> = schema.required.iter().cloned().collect();
            for (key, value) in &schema.properties {
                if is_required && !required.contains(key) {
                    continue;
                }

                if path.is_empty() {
                    path.push(key.to_string());
                } else {
                    path.push(format!(".{key}"));
                }
                populate_schema_list(list, &value.value, types, seen, path, is_required, prefix);
                path.pop();
            }
        }
        core_entities::service::schema_object::SchemaType::ARRAY => {
            list.push(ParameterPathItem::new(
                path_str,
                "ARRAY".to_owned(),
                Some(prefix_str),
                schema.description.clone(),
            ));
            path.push("[0]".to_owned());
            populate_schema_list(
                list,
                &schema.items.value,
                types,
                seen,
                path,
                is_required,
                prefix,
            );
            path.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use protobuf::EnumOrUnknown;

    use super::*;

    #[test]
    fn populate_parameter_list_does_not_panic_on_an_unrecognized_parameter_type() {
        let mut list = Vec::new();

        populate_parameter_list(
            &mut list,
            EnumOrUnknown::from_i32(999),
            "name",
            "description",
        );

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].type_, "UNKNOWN");
    }

    #[test]
    fn populate_schema_object_list_does_not_panic_on_an_unrecognized_schema_type() {
        let schema = core_entities::service::SchemaObject {
            type_: EnumOrUnknown::from_i32(999),
            ..Default::default()
        };
        let types = HashMap::new();
        let mut seen = HashMap::new();
        let mut path = vec!["name".to_owned()];
        let mut prefix = vec![];
        let mut list = Vec::new();

        populate_schema_object_list(
            &mut list,
            &schema,
            &types,
            &mut seen,
            &mut path,
            false,
            &mut prefix,
        );

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].type_, "UNKNOWN");
    }
}
