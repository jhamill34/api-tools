//! Plain-Rust mirror of `src/proto/service.proto` - every message/enum
//! that's actually referenced anywhere in this workspace outside
//! `entities/core` itself, and nothing else.
//!
//! JSON shape matches `protobuf-json-mapping`'s proto3 canonical JSON
//! output byte-for-byte (see each module's `json_compat_tests`), since
//! `manifest.json`/`config.json` are on-disk, user-authored files
//! (`service_loader::loaders::load_service`/`load_configuration`) and must
//! keep parsing unchanged.
//!
//! Intentionally dropped, because nothing outside `entities/core` ever
//! referenced them: the entire unwired "scripted action chain" cluster
//! (`ChainItem`, `Action`, `Conditional`, `ForEach`,
//! `ServiceGroupFieldOperation`, `ActionParam`, `ServiceId`,
//! `ConditionalVersion`, and `ScriptedAction`'s `outputs`/`chainItems`/
//! `errorChainItems` fields that referenced them) and a handful of orphaned
//! `OpenAPI`-schema leaves (`ServerWithVariables` and `CommonApi`'s
//! `serverWithVariables` oneof variant, `MediaType::AwsEncoding`/
//! `Encoding` and `MediaType`'s `propertiesEncoding` field,
//! `SchemaObject::SchemaObjectDefault`/`AdditionalProperties` and
//! `SchemaObject`'s `default`/`additionalProperties` fields,
//! `Discriminator` and `ComposedSchema`'s `discriminator` field,
//! `NullableInt32` and `SchemaObject`'s `maxItems` field,
//! `Parameter::StyleType` and `Parameter`'s `style` field,
//! `ConfigFieldMetadata` and `SwaggerService`'s `additionalConfigs` field,
//! `APIWrappedService::OpenAIGenerator` and its `openAIGenerator` field).
//!
//! A handful of oneofs that only ever had one live variant (after the
//! drops above, or from the start) were flattened to a plain `Option<T>`
//! field instead of kept as a single-variant enum: `CommonApi`'s `server`
//! (now `base_path`), `SwaggerService::ServiceAuth`'s `config` (now
//! `oauth_config`), `SwaggerOverrides`'s `authOverrides` (now
//! `oauth_config`), `ActionOperation`'s `value` (now `function`), and
//! `FunctionOperation`'s `code` (now `js`). `VersionedServiceTree`'s
//! `version` and `ServiceManifest`'s `value` were kept as (currently
//! single-variant) enums instead, since their `v1`/`v2` naming signals a
//! deliberate version-namespacing scheme meant for future extension, not
//! an accident of an unused sibling being dropped.

mod manifest;
mod openapi;
mod params;
mod util;

pub use manifest::{
    action_service, apiwrapped_service, code_resource, service_manifest, service_manifest_latest,
    swagger_service, versioned_service_tree, workflow_service, ActionService, CodeResource,
    FunctionOperation, OutputSelector, ScriptedAction, ServiceManifest, ServiceManifestLatest,
    ServiceResource, SimpleCodeService, SwaggerOverrides, SwaggerService, VersionedServiceTree,
    WorkflowService, APIWrappedService,
};
pub use openapi::{
    operation, pagination, parameter, schema_object, ApiResponse, ApiResponses, CommonApi,
    ComposedSchema, MediaType, Operation, Pagination, Parameter, RequestBody, Schema, SchemaObject,
    SchemaValue,
};
pub use params::{common_parameter, McOperationParameter, OperationParameter};

#[cfg(test)]
mod json_compat_tests;

