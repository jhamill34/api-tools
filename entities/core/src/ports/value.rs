//! An arbitrary, dynamically-typed value exchanged across the engine's
//! output ports (operation `params`/`options`/results) - the domain
//! concept of "some structured data whose shape isn't known statically",
//! kept separate from any one serialization library's concrete type so a
//! port's contract doesn't dictate how its implementations happen to
//! represent that data internally. The `core_json_compat` crate provides
//! lossless conversion to/from `serde_json::Value` for implementers that
//! want to keep working with JSON internally.

use std::collections::BTreeMap;

/// An arbitrary, dynamically-typed value - isomorphic to JSON's data
/// model, so it round-trips losslessly to/from `serde_json::Value`.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeValue {
    /// The absence of a value.
    Null,

    /// A boolean.
    Bool(bool),

    /// A number.
    Number(Number),

    /// A string.
    String(String),

    /// An ordered list of values.
    Array(Vec<RuntimeValue>),

    /// A string-keyed map of values.
    Object(BTreeMap<String, RuntimeValue>),
}

/// A number, keeping the u64/i64/f64 distinction a value was constructed
/// with rather than collapsing to a single numeric type - an integer
/// beyond `f64`'s 53-bit mantissa (an auth token, a snowflake-style ID)
/// would otherwise silently lose precision on every conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    /// A non-negative integer.
    PosInt(u64),

    /// A negative integer.
    NegInt(i64),

    /// A floating-point number.
    Float(f64),
}
