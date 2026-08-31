//! Shared `serde` helpers used across [`crate::service`] so every message's
//! JSON shape matches `protobuf-json-mapping`'s proto3 canonical output:
//! fields at their default value are omitted, and a `uint64`/`int64` field
//! is written as a JSON string (to avoid precision loss in JS-based
//! consumers), never a bare number.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// `#[serde(skip_serializing_if = "...")]` target: true when `value` is
/// this type's default, so default-valued fields are omitted the way
/// proto3's canonical JSON mapping omits them.
pub(crate) fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// `#[serde(with = "...")]` target for a `u64` field - (de)serializes it as
/// a JSON string, matching proto3's canonical JSON mapping for
/// `uint64`/`int64`/`fixed64`/`sfixed64` fields (unlike `u32`, which is a
/// bare JSON number).
pub(crate) mod u64_as_string {
    use super::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        value.to_string().serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
