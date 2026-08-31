//! Parameter/output descriptors shared by the non-`OpenAPI` manifest kinds
//! (`Action`, `ApiWrapped`, `SimpleCode`, `ScriptedAction`).

use serde::{Deserialize, Serialize};

use super::util::is_default;

/// Types nested under the (fieldless) `CommonParameter` message - kept only
/// for [`common_parameter::ParameterType`], the one thing anything actually
/// references.
pub mod common_parameter {
    use serde::{Deserialize, Serialize};

    /// The JSON type of an [`super::OperationParameter`]/
    /// [`super::McOperationParameter`].
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub enum ParameterType {
        /// No type set.
        #[default]
        #[serde(rename = "UNSET")]
        Unset,
        /// `string`.
        #[serde(rename = "STRING")]
        String,
        /// `integer`.
        #[serde(rename = "INTEGER")]
        Integer,
        /// `number`.
        #[serde(rename = "NUMBER")]
        Number,
        /// `boolean`.
        #[serde(rename = "BOOLEAN")]
        Boolean,
        /// `object`.
        #[serde(rename = "OBJECT")]
        Object,
        /// `array`.
        #[serde(rename = "ARRAY")]
        Array,
    }
}

/// An input parameter to an `Action`/`ApiWrapped`/`SimpleCode` operation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationParameter {
    /// The parameter's name.
    #[serde(default, skip_serializing_if = "is_default")]
    pub name: String,
    /// The parameter's description.
    #[serde(default, skip_serializing_if = "is_default")]
    pub description: String,
    /// The parameter's type.
    #[serde(default, skip_serializing_if = "is_default")]
    pub r#type: common_parameter::ParameterType,
    /// Whether the parameter is required.
    #[serde(default, skip_serializing_if = "is_default")]
    pub required: bool,
    /// The parameter's display name.
    #[serde(default, skip_serializing_if = "is_default")]
    pub pretty_name: String,
}

/// An output descriptor for an `Action`/`ApiWrapped`/`SimpleCode` operation
/// (`Mc` = "manifest-computed": machine-readable metadata about a value the
/// operation produces, not an input it takes).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McOperationParameter {
    /// The output's name.
    #[serde(default, skip_serializing_if = "is_default")]
    pub name: String,
    /// The output's description.
    #[serde(default, skip_serializing_if = "is_default")]
    pub description: String,
    /// The output's type.
    #[serde(default, skip_serializing_if = "is_default")]
    pub r#type: common_parameter::ParameterType,
    /// The output's display name.
    #[serde(default, skip_serializing_if = "is_default")]
    pub pretty_name: String,
}
