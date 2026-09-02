//! Local JSON-schema inference and merging: the `schema`/`merge` subcommands
//! never talk to `apid` at all, unlike everything in [`crate::engine`].

use std::{collections::HashMap, fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::io_util::read_lines_from_stdin;

/// Infers a YAML schema from a JSON example payload (`input`, or read from
/// stdin if omitted) via [`schemaify`] and prints it.
///
/// # Errors
pub fn handle_schema_convert(input: Option<String>) -> anyhow::Result<()> {
    let input = if let Some(input) = input {
        fs::read_to_string(Path::new(&input))?
    } else {
        read_lines_from_stdin()?
    };

    let input = serde_json::from_str(&input)?;
    let schema = schemaify(&input);

    let schema = serde_yaml::to_string(&schema)?;

    println!("{schema}");

    Ok(())
}

/// Reads two YAML schema files and prints their [`merge`]d union.
///
/// # Errors
pub fn handle_schema_merge(left: &str, right: &str) -> anyhow::Result<()> {
    let left = fs::read_to_string(Path::new(&left))?;
    let left: Schema = serde_yaml::from_str(&left)?;

    let right = fs::read_to_string(Path::new(&right))?;
    let right: Schema = serde_yaml::from_str(&right)?;

    let merged = merge(left, right);
    let merged = serde_yaml::to_string(&merged)?;

    println!("{merged}");

    Ok(())
}

/// An inferred JSON schema: either a single concrete type, or a `oneOf`
/// composition when the same position held incompatible types.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
enum Schema {
    /// A single concrete type.
    Single(SchemaObject),

    /// A `oneOf` composition of multiple possible types.
    Composite(SchemaComposite),
}

/// A `oneOf` composition of possible schemas.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SchemaComposite {
    /// The possible schemas, deduplicated.
    one_of: Vec<Schema>,
}

/// A single concrete inferred type.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
enum SchemaObject {
    /// Inferred from a JSON `null`.
    Null,

    /// Inferred from a JSON boolean.
    Boolean,

    ///s
    Number,

    /// Inferred from a JSON string.
    String,

    /// Inferred from a JSON object.
    Object {
        /// Each property's inferred schema, keyed by name.
        properties: HashMap<String, Schema>,
    },

    /// Inferred from a JSON array, with every element's schema merged
    /// into one.
    Array {
        /// The merged element schema.
        items: Box<Schema>,
    },
}

/// Infers a [`Schema`] from a JSON value: a concrete type for a scalar, a
/// per-key schema for an object, or the [`merge`]d schema of every element
/// for an array (an empty array infers an empty object, since there's
/// nothing to merge).
fn schemaify(value: &serde_json::Value) -> Schema {
    match value {
        serde_json::Value::Null => Schema::Single(SchemaObject::Null),
        serde_json::Value::Bool(_) => Schema::Single(SchemaObject::Boolean),
        serde_json::Value::Number(_) => Schema::Single(SchemaObject::Number),
        serde_json::Value::String(_) => Schema::Single(SchemaObject::String),
        serde_json::Value::Object(obj) => {
            let mut properties = HashMap::new();

            for (key, value) in obj {
                properties.insert(key.clone(), schemaify(value));
            }

            Schema::Single(SchemaObject::Object { properties })
        }
        serde_json::Value::Array(arr) => {
            let result = arr.iter().map(schemaify).reduce(merge);

            if let Some(result) = result {
                Schema::Single(SchemaObject::Array {
                    items: Box::new(result),
                })
            } else {
                Schema::Single(SchemaObject::Object {
                    properties: HashMap::new(),
                })
            }
        }
    }
}

/// Merges two schemas into one: identical schemas merge to themselves;
/// two objects merge property-by-property (a property present on only one
/// side is kept as-is); two arrays merge their item schemas; anything else
/// incompatible becomes (or extends) a `oneOf` composition of the
/// distinct schemas seen.
fn merge(left: Schema, right: Schema) -> Schema {
    if left == right {
        left
    } else {
        match &left {
            Schema::Single(SchemaObject::Object { properties }) => match &right {
                Schema::Single(SchemaObject::Object {
                    properties: right_properties,
                }) => {
                    let mut existing = HashMap::new();

                    for (key, value) in properties {
                        if let Some(right_value) = right_properties.get(key) {
                            existing.insert(key.clone(), merge(value.clone(), right_value.clone()));
                        } else {
                            existing.insert(key.clone(), value.clone());
                        }
                    }

                    for (key, value) in right_properties {
                        if !existing.contains_key(key) {
                            existing.insert(key.clone(), value.clone());
                        }
                    }

                    Schema::Single(SchemaObject::Object {
                        properties: existing,
                    })
                }
                Schema::Composite(SchemaComposite { one_of }) => {
                    let mut one_of = one_of.clone();

                    if !one_of.contains(&left) {
                        one_of.push(left);
                    }

                    Schema::Composite(SchemaComposite { one_of })
                }
                Schema::Single(_) => Schema::Composite(SchemaComposite {
                    one_of: vec![left, right],
                }),
            },
            Schema::Single(SchemaObject::Array { items }) => match &right {
                Schema::Single(SchemaObject::Array { items: right_items }) => {
                    Schema::Single(SchemaObject::Array {
                        items: Box::new(merge((**items).clone(), (**right_items).clone())),
                    })
                }
                Schema::Composite(SchemaComposite { one_of }) => {
                    let mut one_of = one_of.clone();

                    if !one_of.contains(&left) {
                        one_of.push(left);
                    }

                    Schema::Composite(SchemaComposite { one_of })
                }
                Schema::Single(_) => Schema::Composite(SchemaComposite {
                    one_of: vec![left, right],
                }),
            },
            Schema::Composite(SchemaComposite { one_of }) => match &right {
                Schema::Single(_) => {
                    let mut one_of = one_of.clone();
                    if !one_of.contains(&right) {
                        one_of.push(right);
                    }

                    Schema::Composite(SchemaComposite { one_of })
                }
                Schema::Composite(SchemaComposite {
                    one_of: right_one_of,
                }) => {
                    let mut one_of = one_of.clone();
                    for right_value in right_one_of {
                        if !one_of.contains(right_value) {
                            one_of.push(right_value.clone());
                        }
                    }

                    Schema::Composite(SchemaComposite { one_of })
                }
            },
            Schema::Single(_) => match &right {
                Schema::Single(_) => Schema::Composite(SchemaComposite {
                    one_of: vec![left, right],
                }),
                Schema::Composite(SchemaComposite { one_of }) => {
                    let mut one_of = one_of.clone();
                    if !one_of.contains(&left) {
                        one_of.push(left.clone());
                    }

                    Schema::Composite(SchemaComposite { one_of })
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemaify_infers_scalar_types() {
        assert!(matches!(
            schemaify(&serde_json::json!(null)),
            Schema::Single(SchemaObject::Null)
        ));
        assert!(matches!(
            schemaify(&serde_json::json!(true)),
            Schema::Single(SchemaObject::Boolean)
        ));
        assert!(matches!(
            schemaify(&serde_json::json!(1)),
            Schema::Single(SchemaObject::Number)
        ));
        assert!(matches!(
            schemaify(&serde_json::json!("hello")),
            Schema::Single(SchemaObject::String)
        ));
    }

    #[test]
    fn schemaify_infers_an_empty_object_for_an_empty_array() {
        let schema = schemaify(&serde_json::json!([]));

        assert!(matches!(
            schema,
            Schema::Single(SchemaObject::Object { properties }) if properties.is_empty()
        ));
    }

    #[test]
    fn merge_of_identical_schemas_is_that_schema() {
        let schema = schemaify(&serde_json::json!("hello"));

        assert!(merge(schema.clone(), schema.clone()) == schema);
    }

    #[test]
    fn merge_of_incompatible_scalars_becomes_a_one_of() {
        let string_schema = schemaify(&serde_json::json!("hello"));
        let number_schema = schemaify(&serde_json::json!(1));

        let merged = merge(string_schema, number_schema);

        assert!(matches!(
            merged,
            Schema::Composite(SchemaComposite { one_of }) if one_of.len() == 2
        ));
    }

    #[test]
    fn merge_of_objects_unions_their_properties() {
        let left = schemaify(&serde_json::json!({ "a": 1 }));
        let right = schemaify(&serde_json::json!({ "b": "x" }));

        let merged = merge(left, right);

        let Schema::Single(SchemaObject::Object { properties }) = merged else {
            panic!("expected an object schema");
        };

        assert!(matches!(
            properties.get("a"),
            Some(Schema::Single(SchemaObject::Number))
        ));
        assert!(matches!(
            properties.get("b"),
            Some(Schema::Single(SchemaObject::String))
        ));
    }
}
