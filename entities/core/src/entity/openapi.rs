//! The `OpenAPI`-shaped subset of a loaded service's common API definition:
//! paths, operations, schemas, and pagination strategies.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::util::is_default;

/// A service's parsed common API definition: its base URL, operations, and
/// the schemas they reference.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommonApi {
    /// The API's base URL, if it's a plain fixed string (the only
    /// server-address shape still in use - see the crate-level doc comment
    /// on [`crate::service`] for why `serverWithVariables` was dropped).
    #[serde(default, skip_serializing_if = "is_default")]
    pub base_path: Option<String>,

    /// Operation ID to its definition.
    #[serde(default, skip_serializing_if = "is_default")]
    pub operations: HashMap<String, Operation>,

    /// Schema name to its definition.
    #[serde(default, skip_serializing_if = "is_default")]
    pub schemas: HashMap<String, Schema>,

    /// The API's display title.
    #[serde(default, skip_serializing_if = "is_default")]
    pub title: String,

    /// The API's description.
    #[serde(default, skip_serializing_if = "is_default")]
    pub description: String,
}

/// A single `OpenAPI` operation (one HTTP verb against one path).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    /// The request path template.
    #[serde(default, skip_serializing_if = "is_default")]
    pub path: String,

    /// The HTTP method.
    #[serde(default, skip_serializing_if = "is_default")]
    pub method: operation::HttpMethodType,

    /// The operation's parameters.
    #[serde(default, skip_serializing_if = "is_default")]
    pub parameter: Vec<Parameter>,

    /// The request body's shape, for methods that send one.
    #[serde(default, skip_serializing_if = "is_default")]
    pub request_body: Option<RequestBody>,

    /// The operation's possible responses.
    #[serde(default, skip_serializing_if = "is_default")]
    pub api_responses: Option<ApiResponses>,

    /// The operation's identifier, unique within its `CommonApi`.
    #[serde(default, skip_serializing_if = "is_default")]
    pub id: String,

    /// The operation's description.
    #[serde(default, skip_serializing_if = "is_default")]
    pub description: String,

    /// How to page through this operation's results, if it's paginated.
    #[serde(default, skip_serializing_if = "is_default")]
    pub pagination: Option<Pagination>,

    /// A short summary of the operation.
    #[serde(default, skip_serializing_if = "is_default")]
    pub summary: String,
}

/// Types nested under [`Operation`].
pub mod operation {
    use serde::{Deserialize, Serialize};

    /// The HTTP method an [`super::Operation`] is invoked with.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub enum HttpMethodType {
        /// No method set.
        #[default]
        #[serde(rename = "HTTP_METHOD_TYPE_NONE")]
        None,
        /// `POST`.
        #[serde(rename = "POST")]
        Post,
        /// `GET`.
        #[serde(rename = "GET")]
        Get,
        /// `PUT`.
        #[serde(rename = "PUT")]
        Put,
        /// `PATCH`.
        #[serde(rename = "PATCH")]
        Patch,
        /// `DELETE`.
        #[serde(rename = "DELETE")]
        Delete,
        /// `HEAD`.
        #[serde(rename = "HEAD")]
        Head,
        /// `OPTIONS`.
        #[serde(rename = "OPTIONS")]
        Options,
        /// `TRACE`.
        #[serde(rename = "TRACE")]
        Trace,
    }
}

/// How to page through a paginated operation's results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Pagination {
    /// Cursor-per-page, driven by one or more cursor values.
    MultiCursor(pagination::MultiCursor),
    /// A page-number/page-size scheme.
    PageOffset(pagination::PageOffset),
    /// A record-offset/limit scheme.
    Offset(pagination::Offset),
    /// A server-provided "next page" URL.
    NextUrl(pagination::NextUrl),
    /// No pagination; every result comes back in one response.
    Unpaginated(pagination::Unpaginated),
}

/// Types nested under [`Pagination`].
pub mod pagination {
    use serde::{Deserialize, Serialize};

    use super::is_default;

    /// A JMESPath- or column-addressed path into a response body.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum ExtendedPath {
        /// A literal, dot-separated column path.
        ColumnPath(String),
        /// A `JMESPath` expression.
        JmesPath(String),
    }

    /// Cursor-per-page pagination, possibly with multiple cursor values in
    /// play at once.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct MultiCursor {
        /// Where in the response each next cursor value is read from.
        #[serde(default, skip_serializing_if = "is_default")]
        pub cursors_path: Vec<ExtendedPath>,
        /// The request parameter each cursor value is sent back as.
        #[serde(default, skip_serializing_if = "is_default")]
        pub cursors_param: Vec<String>,
        /// The request parameter the page size limit is sent as.
        #[serde(default, skip_serializing_if = "is_default")]
        pub limit_param: String,
        /// The maximum page size to request, if capped.
        #[serde(default, skip_serializing_if = "is_default")]
        pub max_limit: Option<i32>,
        /// Where in the response the page's results are read from.
        #[serde(default, skip_serializing_if = "is_default")]
        pub results_path: Option<ExtendedPath>,
        /// Whether a missing `results_path` is an error rather than "no more
        /// results."
        #[serde(default, skip_serializing_if = "is_default")]
        pub error_on_path_not_found: Option<bool>,
    }

    /// Page-number/page-size pagination.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PageOffset {
        /// The request parameter the page number is sent as.
        #[serde(default, skip_serializing_if = "is_default")]
        pub page_offset_param: String,
        /// The page number to start from.
        #[serde(default, skip_serializing_if = "is_default")]
        pub start_page: Option<i32>,
        /// The request parameter the page size limit is sent as.
        #[serde(default, skip_serializing_if = "is_default")]
        pub limit_param: String,
        /// The maximum page size to request, if capped.
        #[serde(default, skip_serializing_if = "is_default")]
        pub max_limit: Option<i32>,
        /// Where in the response the page's results are read from.
        #[serde(default, skip_serializing_if = "is_default")]
        pub results_path: Option<ExtendedPath>,
        /// Whether a missing `results_path` is an error rather than "no more
        /// results."
        #[serde(default, skip_serializing_if = "is_default")]
        pub error_on_path_not_found: Option<bool>,
    }

    /// Record-offset/limit pagination.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Offset {
        /// The request parameter the record offset is sent as.
        #[serde(default, skip_serializing_if = "is_default")]
        pub offset_param: String,
        /// The request parameter the page size limit is sent as.
        #[serde(default, skip_serializing_if = "is_default")]
        pub limit_param: String,
        /// The maximum page size to request, if capped.
        #[serde(default, skip_serializing_if = "is_default")]
        pub max_limit: Option<i32>,
        /// Where in the response the page's results are read from.
        #[serde(default, skip_serializing_if = "is_default")]
        pub results_path: Option<ExtendedPath>,
        /// Whether a missing `results_path` is an error rather than "no more
        /// results."
        #[serde(default, skip_serializing_if = "is_default")]
        pub error_on_path_not_found: Option<bool>,
    }

    /// Server-provided "next page" URL pagination.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct NextUrl {
        /// Where in the response the next page's URL is read from.
        #[serde(default, skip_serializing_if = "is_default")]
        pub next_url_path: Option<ExtendedPath>,
        /// The request parameter the page size limit is sent as.
        #[serde(default, skip_serializing_if = "is_default")]
        pub limit_param: String,
        /// The maximum page size to request, if capped.
        #[serde(default, skip_serializing_if = "is_default")]
        pub max_limit: Option<i32>,
        /// Where in the response the page's results are read from.
        #[serde(default, skip_serializing_if = "is_default")]
        pub results_path: Option<ExtendedPath>,
        /// Whether a missing `results_path` is an error rather than "no more
        /// results."
        #[serde(default, skip_serializing_if = "is_default")]
        pub error_on_path_not_found: Option<bool>,
    }

    /// No pagination; every result comes back in one response.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Unpaginated {
        /// Where in the response the results are read from.
        #[serde(default, skip_serializing_if = "is_default")]
        pub results_path: Option<ExtendedPath>,
        /// Whether a missing `results_path` is an error.
        #[serde(default, skip_serializing_if = "is_default")]
        pub error_on_path_not_found: Option<bool>,
    }
}

/// A single `OpenAPI` operation parameter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parameter {
    /// The parameter's name.
    #[serde(default, skip_serializing_if = "is_default")]
    pub name: String,
    /// The parameter's description.
    #[serde(default, skip_serializing_if = "is_default")]
    pub description: String,
    /// Whether the parameter is required.
    #[serde(default, skip_serializing_if = "is_default")]
    pub required: bool,
    /// The parameter's value schema.
    #[serde(default, skip_serializing_if = "is_default")]
    pub schema: Option<Schema>,
    /// Where the parameter is sent.
    #[serde(default, skip_serializing_if = "is_default")]
    pub r#in: parameter::InType,
    /// Whether the parameter is exploded (see the `OpenAPI` spec's
    /// `explode` keyword).
    #[serde(default, skip_serializing_if = "is_default")]
    pub explode: bool,
    /// The parameter's default value, if any, as a string.
    #[serde(default, skip_serializing_if = "is_default")]
    pub default_value: String,
}

/// Types nested under [`Parameter`].
pub mod parameter {
    use serde::{Deserialize, Serialize};

    /// Where a [`super::Parameter`] is sent on the request.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub enum InType {
        /// No location set.
        #[default]
        #[serde(rename = "IN_TYPE_NONE")]
        None,
        /// A query-string parameter.
        #[serde(rename = "QUERY")]
        Query,
        /// A header.
        #[serde(rename = "HEADER")]
        Header,
        /// A path segment.
        #[serde(rename = "PATH")]
        Path,
        /// A cookie.
        #[serde(rename = "COOKIE")]
        Cookie,
        /// Multiple headers at once.
        #[serde(rename = "HEADERS")]
        Headers,
    }
}

/// A request body's shape.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestBody {
    /// Whether the request body is required.
    #[serde(default, skip_serializing_if = "is_default")]
    pub required: bool,
    /// Media type to its schema.
    #[serde(default, skip_serializing_if = "is_default")]
    pub content: HashMap<String, MediaType>,
    /// The request body's description.
    #[serde(default, skip_serializing_if = "is_default")]
    pub description: String,
    /// A default, empty-request-shaped body, serialized as a JSON string.
    #[serde(default, skip_serializing_if = "is_default")]
    pub default_empty_body: Option<String>,
}

/// An operation's possible responses.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiResponses {
    /// The response to use when no status-code-specific response matches.
    #[serde(default, skip_serializing_if = "is_default")]
    pub default: Option<ApiResponse>,
    /// Status code to its response.
    #[serde(default, skip_serializing_if = "is_default")]
    pub api_responses: HashMap<String, ApiResponse>,
}

/// A single possible response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiResponse {
    /// Media type to its schema.
    #[serde(default, skip_serializing_if = "is_default")]
    pub content: HashMap<String, MediaType>,
}

/// A single media type's schema (e.g. `application/json`'s body shape).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaType {
    /// The media type's schema.
    #[serde(default, skip_serializing_if = "is_default")]
    pub schema: Option<Schema>,
}

/// A JSON Schema value: either a `$ref`, an inline object/array/scalar
/// schema, or a composed (`allOf`/`anyOf`/`oneOf`) schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Schema {
    /// A `$ref` to another schema.
    Ref(String),
    /// An inline schema.
    SchemaObject(SchemaObject),
    /// A schema composed via `allOf`.
    AllOf(ComposedSchema),
    /// A schema composed via `anyOf`.
    AnyOf(ComposedSchema),
    /// A schema composed via `oneOf`.
    OneOf(ComposedSchema),
}

/// An inline JSON Schema object/array/scalar definition.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaObject {
    /// The schema's JSON type.
    #[serde(default, skip_serializing_if = "is_default")]
    pub r#type: schema_object::SchemaType,
    /// The object's required property names.
    #[serde(default, skip_serializing_if = "is_default")]
    pub required: Vec<String>,
    /// Property name to its schema, for an `object`-typed schema.
    #[serde(default, skip_serializing_if = "is_default")]
    pub properties: HashMap<String, Schema>,
    /// The element schema, for an `array`-typed schema.
    #[serde(default, skip_serializing_if = "is_default")]
    pub items: Option<Box<Schema>>,
    /// The value's allowed values, if it's an enum.
    #[serde(default, skip_serializing_if = "is_default")]
    pub possible_values: Vec<String>,
    /// The schema's format hint (e.g. `date-time`).
    #[serde(default, skip_serializing_if = "is_default")]
    pub format: String,
    /// The schema's description.
    #[serde(default, skip_serializing_if = "is_default")]
    pub description: String,
    /// The schema's name.
    #[serde(default, skip_serializing_if = "is_default")]
    pub name: String,
}

/// Types nested under [`SchemaObject`].
pub mod schema_object {
    use serde::{Deserialize, Serialize};

    /// A `SchemaObject`'s JSON type.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub enum SchemaType {
        /// No type set.
        #[default]
        #[serde(rename = "SCHEMA_TYPE_NONE")]
        None,
        /// `string`.
        #[serde(rename = "STRING")]
        String,
        /// `number`.
        #[serde(rename = "NUMBER")]
        Number,
        /// `integer`.
        #[serde(rename = "INTEGER")]
        Integer,
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

/// A schema composed from other schemas via `allOf`/`anyOf`/`oneOf`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposedSchema {
    /// The component schemas.
    #[serde(default, skip_serializing_if = "is_default")]
    pub schema: Vec<Schema>,
}
