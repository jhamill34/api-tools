//!

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail};
use common_data_structures::trie::Trie;
use core_entities::service::VersionedServiceTree;

///
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
                    param.type_.unwrap(),
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

///
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
                    param.type_.unwrap(),
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

///
pub struct ParameterPathItem {
    ///
    pub path: String,

    ///
    pub type_: String,

    ///
    pub context: Option<String>,

    ///
    pub description: String,
}

impl ParameterPathItem {
    ///
    pub fn new(path: String, type_: String, context: Option<String>, description: String) -> Self {
        Self {
            path,
            type_,
            context,
            description,
        }
    }
}

///
pub fn populate_parameter_list(
    list: &mut Vec<ParameterPathItem>,
    param: core_entities::service::common_parameter::ParameterType,
    name: &str,
    description: &str,
) {
    match param {
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

///
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

///
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
    match schema.type_.unwrap() {
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
