//! A loaded service's manifest: which of the supported kinds it is
//! (`Swagger`/`Action`/`ApiWrapped`/`SimpleCode`/`ScriptedAction`/
//! `Workflow`), plus the tree it's stored in alongside its resources and
//! parsed `OpenAPI` definition.

use std::{borrow::Cow, collections::HashMap};

use serde::{Deserialize, Serialize};

use super::{
    openapi::CommonApi,
    params::{McOperationParameter, OperationParameter},
    util::is_default,
};

/// The root of a loaded service: a version tag over its manifest,
/// resources, and parsed `OpenAPI` definition, so a future format change
/// can add a new variant without breaking readers of the old one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VersionedServiceTree {
    /// The tree's version.
    #[serde(flatten, default, skip_serializing_if = "is_default")]
    pub version: Option<versioned_service_tree::Version>,
}

impl VersionedServiceTree {
    /// Returns this tree's version-1 content, or an empty one if the tree
    /// has no version set - every reader in this workspace treats "no
    /// version" as "an empty tree" rather than an error, so this saves
    /// every one of them from re-deriving that default.
    #[must_use]
    pub fn v1(&self) -> Cow<'_, versioned_service_tree::V1> {
        match &self.version {
            Some(versioned_service_tree::Version::V1(v1)) => Cow::Borrowed(v1),
            None => Cow::Owned(versioned_service_tree::V1::default()),
        }
    }
}

/// Types nested under [`VersionedServiceTree`].
pub mod versioned_service_tree {
    use std::borrow::Cow;

    use serde::{Deserialize, Serialize};

    use super::{is_default, CommonApi, ServiceManifest, ServiceResource};

    /// A [`super::VersionedServiceTree`]'s version.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub enum Version {
        /// The only version defined so far.
        #[serde(rename = "v1")]
        V1(V1),
    }

    /// The tree's actual content, as of version 1.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct V1 {
        /// The service's manifest.
        #[serde(default, skip_serializing_if = "is_default")]
        pub manifest: Option<ServiceManifest>,
        /// The service's action-script resources, if it has any.
        #[serde(default, skip_serializing_if = "is_default")]
        pub resources: Vec<ServiceResource>,
        /// The service's parsed `OpenAPI` definition, if it's `Swagger`-kind.
        #[serde(default, skip_serializing_if = "is_default")]
        pub common_api: Option<CommonApi>,
    }

    impl V1 {
        /// Returns this tree's version-2 manifest content, or an empty one
        /// if it has no manifest (or an unversioned one) - see
        /// [`super::VersionedServiceTree::v1`]'s doc comment for why.
        #[must_use]
        pub fn manifest_latest(&self) -> Cow<'_, super::ServiceManifestLatest> {
            self.manifest.as_ref().map_or_else(
                || Cow::Owned(super::ServiceManifestLatest::default()),
                super::ServiceManifest::v2,
            )
        }
    }
}

/// A single action-script resource (e.g. an `Action` operation's JS source
/// file) resolved relative to the manifest that references it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceResource {
    /// The resource's path, relative to the manifest.
    #[serde(default, skip_serializing_if = "is_default")]
    pub relative_path: String,
    /// The resource's content.
    #[serde(default, skip_serializing_if = "is_default")]
    pub content: String,
}

/// A version tag over [`ServiceManifestLatest`], for the same forward-
/// compatibility reason as [`VersionedServiceTree`]'s own version tag.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceManifest {
    /// The manifest's version.
    #[serde(flatten, default, skip_serializing_if = "is_default")]
    pub value: Option<service_manifest::Value>,
}

impl ServiceManifest {
    /// Returns this manifest's version-2 content, or an empty one if the
    /// manifest has no version set - see [`VersionedServiceTree::v1`]'s doc
    /// comment for why.
    #[must_use]
    pub fn v2(&self) -> Cow<'_, ServiceManifestLatest> {
        match &self.value {
            Some(service_manifest::Value::V2(latest)) => Cow::Borrowed(latest),
            None => Cow::Owned(ServiceManifestLatest::default()),
        }
    }
}

/// Types nested under [`ServiceManifest`].
pub mod service_manifest {
    use serde::{Deserialize, Serialize};

    use super::ServiceManifestLatest;

    /// A [`super::ServiceManifest`]'s version.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub enum Value {
        /// The only version defined so far.
        #[serde(rename = "v2")]
        V2(ServiceManifestLatest),
    }
}

/// Which of the supported kinds a service is, plus its display metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceManifestLatest {
    /// Which kind this manifest is, and that kind's own definition.
    #[serde(flatten, default, skip_serializing_if = "is_default")]
    pub value: Option<service_manifest_latest::Value>,
    /// The service's description.
    #[serde(default, skip_serializing_if = "is_default")]
    pub description: String,
    /// The service's display name.
    #[serde(default, skip_serializing_if = "is_default")]
    pub display_name: String,
}

/// Types nested under [`ServiceManifestLatest`].
pub mod service_manifest_latest {
    use serde::{Deserialize, Serialize};

    use super::{
        is_default, APIWrappedService, ActionService, ScriptedAction, SimpleCodeService,
        SwaggerService, WorkflowService,
    };

    /// Which kind a [`super::ServiceManifestLatest`] is.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Value {
        /// An `OpenAPI`-backed connector.
        Swagger(Box<SwaggerService>),
        /// A hand-written-script-backed connector.
        Action(ActionService),
        /// A wrapper around another already-registered operation.
        ApiWrapped(APIWrappedService),
        /// A single script with no `OpenAPI` backing.
        SimpleCode(SimpleCodeService),
        /// A chained sequence of actions (see [`ScriptedAction`]'s own doc
        /// comment for its current status).
        ScriptedAction(ScriptedAction),
        /// A Lua workflow (see [`WorkflowService`]'s own doc comment).
        Workflow(WorkflowService),
    }

    /// OAuth 2.0 authorization-code-flow configuration, shared by
    /// [`super::SwaggerService`]'s auth config and [`super::SwaggerOverrides`].
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct OAuthConfig {
        /// The config's display name.
        #[serde(default, skip_serializing_if = "is_default")]
        pub name: String,
        /// The authorization endpoint URI.
        #[serde(default, skip_serializing_if = "is_default")]
        pub auth_uri: String,
        /// The access-token endpoint URI.
        #[serde(default, skip_serializing_if = "is_default")]
        pub access_token_uri: String,
        /// The OAuth `response_type` value.
        #[serde(default, skip_serializing_if = "is_default")]
        pub response_type: String,
        /// The OAuth `access_type` value.
        #[serde(default, skip_serializing_if = "is_default")]
        pub access_type: String,
        /// The OAuth `prompt` value.
        #[serde(default, skip_serializing_if = "is_default")]
        pub prompt: String,
        /// A link to the provider's OAuth documentation.
        #[serde(default, skip_serializing_if = "is_default")]
        pub oauth_documentation: String,
        /// The HTTP method used to request an access token.
        #[serde(default, skip_serializing_if = "is_default")]
        pub access_token_method: String,
        /// The OAuth `scope` value.
        #[serde(default, skip_serializing_if = "is_default")]
        pub scope: String,
        /// Where the access-token request's parameters are sent.
        #[serde(default, skip_serializing_if = "is_default")]
        pub parameter_location: oauth_config::ParameterLocation,
        /// Whether the access-token request needs `Authorization: Basic`.
        #[serde(default, skip_serializing_if = "is_default")]
        pub needs_basic_auth_header: bool,
        /// Where in the access-token response the token is read from.
        #[serde(default, skip_serializing_if = "is_default")]
        pub access_token_path: String,
        /// Group-level credential sharing configuration.
        #[serde(default, skip_serializing_if = "is_default")]
        pub enable_group_credentials: String,
        /// The OAuth `audience` value.
        #[serde(default, skip_serializing_if = "is_default")]
        pub audience: String,
    }

    /// Types nested under [`OAuthConfig`].
    pub mod oauth_config {
        use serde::{Deserialize, Serialize};

        /// Where an [`super::OAuthConfig`] request's parameters are sent.
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
        pub enum ParameterLocation {
            /// As query-string parameters.
            #[default]
            #[serde(rename = "QUERY")]
            Query,
            /// In the request body.
            #[serde(rename = "BODY")]
            Body,
        }
    }
}

/// An `OpenAPI`-backed connector.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwaggerService {
    /// How the connector authenticates.
    #[serde(default, skip_serializing_if = "is_default")]
    pub auth: Option<swagger_service::ServiceAuth>,
    /// Where the connector's `OpenAPI` document (and any action-script
    /// resources) are resolved relative to.
    #[serde(default, skip_serializing_if = "is_default")]
    pub source: String,
    /// The connector's base URL.
    #[serde(default, skip_serializing_if = "is_default")]
    pub url: String,
    /// `{{placeholder}}` name to its substituted value, for `url` and OAuth
    /// config fields.
    #[serde(default, skip_serializing_if = "is_default")]
    pub server_variables: HashMap<String, String>,
}

/// Types nested under [`SwaggerService`].
pub mod swagger_service {
    use std::collections::HashMap;

    use serde::{Deserialize, Serialize};

    use super::{is_default, service_manifest_latest::OAuthConfig};

    /// How a [`super::SwaggerService`] authenticates.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ServiceAuth {
        /// The auth scheme.
        #[serde(default, skip_serializing_if = "is_default")]
        pub r#type: service_auth::Type,
        /// Auth-scheme-specific parameter name to its value.
        #[serde(default, skip_serializing_if = "is_default")]
        pub params: HashMap<String, service_auth::AuthParam>,
        /// Whether the user must supply their own auth settings before this
        /// connector can run.
        #[serde(default, skip_serializing_if = "is_default")]
        pub auth_settings_required: bool,
        /// The auth scheme's description.
        #[serde(default, skip_serializing_if = "is_default")]
        pub description: String,
        /// OAuth configuration, for `type: OAUTH`.
        #[serde(default, skip_serializing_if = "is_default")]
        pub oauth_config: Option<OAuthConfig>,
    }

    /// Types nested under [`ServiceAuth`].
    pub mod service_auth {
        use serde::{Deserialize, Serialize};

        /// A [`super::ServiceAuth`]'s auth scheme.
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
        pub enum Type {
            /// No scheme set.
            #[default]
            #[serde(rename = "UNSET")]
            Unset,
            /// A static header.
            #[serde(rename = "HEADER")]
            Header,
            /// OAuth 2.0 authorization-code flow.
            #[serde(rename = "OAUTH")]
            Oauth,
            /// A static query parameter.
            #[serde(rename = "PARAMETER")]
            Parameter,
            /// A static path segment.
            #[serde(rename = "PATH")]
            Path,
            /// Basic auth.
            #[serde(rename = "BASIC")]
            Basic,
            /// Multiple static headers at once.
            #[serde(rename = "MULTIHEADER")]
            MultiHeader,
        }

        /// One configured auth parameter's value.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub enum AuthParam {
            /// A single string value.
            #[serde(rename = "string")]
            Str(String),
            /// Multiple header values at once.
            MultiHeaderAuth(MultiHeaderAuth),
        }

        impl AuthParam {
            /// Returns the string value, or `""` if this param is a
            /// [`AuthParam::MultiHeaderAuth`] instead.
            #[must_use]
            pub fn as_str(&self) -> &str {
                match self {
                    Self::Str(s) => s,
                    Self::MultiHeaderAuth(_) => "",
                }
            }

            /// Returns the multi-header values, if that's this param's
            /// shape.
            #[must_use]
            pub fn as_multi_header_auth(&self) -> Option<&MultiHeaderAuth> {
                match self {
                    Self::MultiHeaderAuth(v) => Some(v),
                    Self::Str(_) => None,
                }
            }
        }

        /// Multiple static header values, for `type: MULTIHEADER`.
        #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
        pub struct MultiHeaderAuth {
            /// The header values, in header-definition order.
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            pub strings: Vec<String>,
        }
    }
}

/// A hand-written-script-backed connector.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionService {
    /// The service's operations.
    #[serde(default, skip_serializing_if = "is_default")]
    pub operations: Vec<action_service::ActionOperation>,
    /// Where the operations' action-script resources are resolved relative
    /// to.
    #[serde(default, skip_serializing_if = "is_default")]
    pub source: String,
}

/// Types nested under [`ActionService`].
pub mod action_service {
    use serde::{Deserialize, Serialize};

    use super::{is_default, FunctionOperation};

    /// A single operation of an [`super::ActionService`].
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ActionOperation {
        /// The operation's identifier.
        #[serde(default, skip_serializing_if = "is_default")]
        pub id: String,
        /// The operation's description.
        #[serde(default, skip_serializing_if = "is_default")]
        pub description: String,
        /// The operation's implementation.
        #[serde(default, skip_serializing_if = "is_default")]
        pub function: Option<FunctionOperation>,
    }
}

/// Override configuration applied on top of a loaded `Swagger` manifest
/// (see `service_loader::merge`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwaggerOverrides {
    /// An OAuth config to overlay onto the loaded manifest's own.
    #[serde(default, skip_serializing_if = "is_default")]
    pub oauth_config: Option<service_manifest_latest::OAuthConfig>,
    /// A base URL to overlay onto the loaded manifest's own.
    #[serde(default, skip_serializing_if = "is_default")]
    pub base_url: String,
    /// `{{placeholder}}` overrides to overlay onto the loaded manifest's
    /// own.
    #[serde(default, skip_serializing_if = "is_default")]
    pub server_variables: HashMap<String, String>,
}

/// A single operation of an [`ActionService`]: hand-written source in one
/// scripting language.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionOperation {
    /// The operation's parameters.
    #[serde(default, skip_serializing_if = "is_default")]
    pub parameters: Vec<OperationParameter>,
    /// The language the source is written in.
    #[serde(default, skip_serializing_if = "is_default")]
    pub lang: String,
    /// The operation's declared outputs.
    #[serde(default, skip_serializing_if = "is_default")]
    pub outputs: Vec<McOperationParameter>,
    /// The source file's path, relative to the [`ActionService`]'s
    /// `source`.
    #[serde(default, skip_serializing_if = "is_default")]
    pub js: Option<String>,
}

/// A single output field an [`APIWrappedService`] operation selects out of
/// the wrapped operation's result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputSelector {
    /// The output field's name.
    #[serde(default, skip_serializing_if = "is_default")]
    pub name: String,
    /// A `JMESPath` expression selecting the field out of the wrapped
    /// operation's result.
    #[serde(default, skip_serializing_if = "is_default")]
    pub jmes_path_selector: String,
}

/// A wrapper around another already-registered operation: invokes it and
/// narrows the result to selected output fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct APIWrappedService {
    /// The wrapped operation's service identifier.
    #[serde(default, skip_serializing_if = "is_default")]
    pub connector_id: String,
    /// The wrapped operation's identifier.
    #[serde(default, skip_serializing_if = "is_default")]
    pub connector_operation: String,
    /// This operation's own parameters, each mapped onto one of the wrapped
    /// operation's.
    #[serde(default, skip_serializing_if = "is_default")]
    pub inputs: Vec<apiwrapped_service::Parameter>,
    /// Which fields of the wrapped operation's result to keep.
    #[serde(default, skip_serializing_if = "is_default")]
    pub output_selectors: Vec<OutputSelector>,
}

/// Types nested under [`APIWrappedService`].
pub mod apiwrapped_service {
    use serde::{Deserialize, Serialize};

    use super::{is_default, OperationParameter};

    /// One of an [`super::APIWrappedService`]'s own input parameters,
    /// mapped onto one of the wrapped operation's parameters.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Parameter {
        /// This operation's own parameter definition.
        #[serde(default, skip_serializing_if = "is_default")]
        pub param: Option<OperationParameter>,
        /// The wrapped operation's parameter name this maps onto.
        #[serde(default, skip_serializing_if = "is_default")]
        pub api_param_name: String,
    }
}

/// A single script's source: either inline or resolved from a resource
/// file, in one language.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeResource {
    /// The script's language.
    #[serde(default, skip_serializing_if = "is_default")]
    pub language: code_resource::Language,
    /// The script's source.
    #[serde(flatten, default, skip_serializing_if = "is_default")]
    pub value: Option<code_resource::Value>,
}

impl CodeResource {
    /// Returns the inline source, or `""` if this resource's source is a
    /// [`code_resource::Value::ResourcePath`] (or unset) instead.
    #[must_use]
    pub fn code_string(&self) -> &str {
        match &self.value {
            Some(code_resource::Value::CodeString(s)) => s,
            _ => "",
        }
    }

    /// Returns the resource path, or `""` if this resource's source is
    /// inline (or unset) instead.
    #[must_use]
    pub fn resource_path(&self) -> &str {
        match &self.value {
            Some(code_resource::Value::ResourcePath(s)) => s,
            _ => "",
        }
    }
}

/// Types nested under [`CodeResource`].
pub mod code_resource {
    use serde::{Deserialize, Serialize};

    /// A [`super::CodeResource`]'s language.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub enum Language {
        /// No language set.
        #[default]
        #[serde(rename = "UNSET")]
        Unset,
        /// `JavaScript`.
        #[serde(rename = "JAVASCRIPT")]
        Javascript,
        /// Python.
        #[serde(rename = "PYTHON")]
        Python,
        /// Lua.
        #[serde(rename = "LUA")]
        Lua,
    }

    /// A [`super::CodeResource`]'s source.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Value {
        /// The source, inline.
        CodeString(String),
        /// A path to a [`super::super::ServiceResource`] holding the
        /// source, relative to the manifest.
        ResourcePath(String),
    }
}

/// A single script with no `OpenAPI` backing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimpleCodeService {
    /// The operation's parameters.
    #[serde(default, skip_serializing_if = "is_default")]
    pub inputs: Vec<OperationParameter>,
    /// The operation's declared outputs.
    #[serde(default, skip_serializing_if = "is_default")]
    pub outputs: Vec<McOperationParameter>,
    /// The operation's source.
    #[serde(default, skip_serializing_if = "is_default")]
    pub code: Option<CodeResource>,
}

/// A coroutine-based, async-native Lua workflow (see
/// `prototypes/workflow_engine` and issue #68/#74) - deliberately Lua-only
/// (no `language` selector, unlike [`CodeResource`]) and deliberately has
/// no `InputPrompter`/`$input`-style field: a workflow is meant to be a
/// deterministic, latency-bounded function of its inputs, never blocked on
/// a human (see issue #29). `timeout_seconds` and `memory_limit_bytes` are
/// enforced per run by the workflow engine itself, not by the daemon
/// around it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowService {
    /// The workflow's Lua source.
    #[serde(flatten, default, skip_serializing_if = "is_default")]
    pub source: Option<workflow_service::Source>,
    /// The run's wall-clock timeout.
    #[serde(default, skip_serializing_if = "is_default")]
    pub timeout_seconds: u32,
    /// The run's memory budget, in bytes.
    #[serde(
        with = "super::util::u64_as_string",
        default,
        skip_serializing_if = "is_default"
    )]
    pub memory_limit_bytes: u64,
}

impl WorkflowService {
    /// Returns the inline Lua source, or `""` if this workflow's source is
    /// a [`workflow_service::Source::ResourcePath`] (or unset) instead.
    #[must_use]
    pub fn code_string(&self) -> &str {
        match &self.source {
            Some(workflow_service::Source::CodeString(s)) => s,
            _ => "",
        }
    }

    /// Returns the resource path, or `""` if this workflow's source is
    /// inline (or unset) instead.
    #[must_use]
    pub fn resource_path(&self) -> &str {
        match &self.source {
            Some(workflow_service::Source::ResourcePath(s)) => s,
            _ => "",
        }
    }
}

/// Types nested under [`WorkflowService`].
pub mod workflow_service {
    use serde::{Deserialize, Serialize};

    /// A [`super::WorkflowService`]'s Lua source.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Source {
        /// The source, inline.
        CodeString(String),
        /// A path to a [`super::ServiceResource`] holding the source,
        /// relative to the manifest.
        ResourcePath(String),
    }
}

/// A chained sequence of actions. Registered as a manifest kind but not
/// currently dispatched to by [`crate`]'s execution engine - see
/// `core_entities::ports::engine::ScriptRunner`'s doc comment.
///
/// The `ChainItem`-based chain-step types this originally referenced
/// (`ChainItem`/`Action`/`Conditional`/`ForEach`/
/// `ServiceGroupFieldOperation`/`ActionParam`, plus this message's own
/// `outputs` field) were dropped when this crate moved off protobuf - none
/// of them were referenced anywhere outside `entities/core` itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptedAction {
    /// The chain's inputs.
    #[serde(default, skip_serializing_if = "is_default")]
    pub inputs: Vec<OperationParameter>,
}
