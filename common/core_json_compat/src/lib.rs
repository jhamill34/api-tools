//! Lossless conversion between [`core_entities::ports::value::RuntimeValue`]
//! and [`serde_json::Value`], for adapters that implement or call one of
//! `core_entities`'s output ports but want to keep working with JSON
//! internally. Kept as a separate crate (rather than living in
//! `core_entities` itself, even behind a feature flag) so `core_entities`
//! never has to depend on `serde_json` at all - the whole point of
//! `RuntimeValue` existing as a distinct type from `serde_json::Value`.

use core_entities::ports::value::{Number, RuntimeValue};

/// Converts a [`RuntimeValue`] into a [`serde_json::Value`].
///
/// A non-finite [`Number::Float`] (`NaN`/infinity) has no JSON
/// representation - `serde_json::Number` can't hold one - so it converts to
/// [`serde_json::Value::Null`] rather than panicking. This can only happen
/// if a `RuntimeValue` was constructed directly with such a float; a value
/// that arrived via [`from_json`] never contains one, since
/// `serde_json::Value` itself can't represent one either.
#[must_use]
pub fn to_json(value: RuntimeValue) -> serde_json::Value {
    match value {
        RuntimeValue::Null => serde_json::Value::Null,
        RuntimeValue::Bool(value) => serde_json::Value::Bool(value),
        RuntimeValue::Number(number) => number_to_json(number),
        RuntimeValue::String(value) => serde_json::Value::String(value),
        RuntimeValue::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(to_json).collect())
        }
        RuntimeValue::Object(entries) => serde_json::Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key, to_json(value)))
                .collect(),
        ),
    }
}

/// Converts a [`serde_json::Value`] into a [`RuntimeValue`].
#[must_use]
pub fn from_json(value: serde_json::Value) -> RuntimeValue {
    match value {
        serde_json::Value::Null => RuntimeValue::Null,
        serde_json::Value::Bool(value) => RuntimeValue::Bool(value),
        serde_json::Value::Number(number) => RuntimeValue::Number(number_from_json(&number)),
        serde_json::Value::String(value) => RuntimeValue::String(value),
        serde_json::Value::Array(values) => {
            RuntimeValue::Array(values.into_iter().map(from_json).collect())
        }
        serde_json::Value::Object(entries) => RuntimeValue::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key, from_json(value)))
                .collect(),
        ),
    }
}

/// Converts a [`Number`] into a [`serde_json::Value::Number`], or
/// [`serde_json::Value::Null`] if it's a non-finite float (see
/// [`to_json`]'s docs).
fn number_to_json(number: Number) -> serde_json::Value {
    let number = match number {
        Number::PosInt(value) => serde_json::Number::from(value),
        Number::NegInt(value) => serde_json::Number::from(value),
        Number::Float(value) => match serde_json::Number::from_f64(value) {
            Some(number) => number,
            None => return serde_json::Value::Null,
        },
    };

    serde_json::Value::Number(number)
}

/// Converts a `serde_json::Number` into a [`Number`], preserving whichever
/// of u64/i64/f64 it was represented as.
fn number_from_json(number: &serde_json::Number) -> Number {
    if let Some(value) = number.as_u64() {
        Number::PosInt(value)
    } else if let Some(value) = number.as_i64() {
        Number::NegInt(value)
    } else {
        // Every `serde_json::Number` that isn't representable as u64/i64 is
        // a finite float - JSON numbers are always finite, so this is
        // infallible in practice.
        Number::Float(number.as_f64().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::{from_json, to_json};

    /// Asserts `to_json(from_json(value.clone())) == value` - the
    /// round-trip property both conversion directions must satisfy.
    fn assert_round_trips(value: &serde_json::Value) {
        let round_tripped = to_json(from_json(value.clone()));
        assert_eq!(
            &round_tripped, value,
            "expected {value:?} to round-trip through RuntimeValue unchanged"
        );
    }

    #[test]
    fn null_round_trips() {
        assert_round_trips(&serde_json::Value::Null);
    }

    #[test]
    fn bools_round_trip() {
        assert_round_trips(&serde_json::json!(true));
        assert_round_trips(&serde_json::json!(false));
    }

    #[test]
    fn strings_round_trip() {
        assert_round_trips(&serde_json::json!("hello"));
        assert_round_trips(&serde_json::json!(""));
    }

    #[test]
    fn u64_max_round_trips_without_precision_loss() {
        assert_round_trips(&serde_json::json!(u64::MAX));
    }

    #[test]
    fn a_value_just_above_f64s_53_bit_mantissa_round_trips_without_precision_loss() {
        let value = (1_u64 << 53) + 1;
        assert_round_trips(&serde_json::json!(value));
    }

    #[test]
    fn negative_integers_round_trip() {
        assert_round_trips(&serde_json::json!(i64::MIN));
        assert_round_trips(&serde_json::json!(-42));
    }

    #[test]
    fn floats_round_trip() {
        assert_round_trips(&serde_json::json!(1.5));
        assert_round_trips(&serde_json::json!(-0.001));
        assert_round_trips(&serde_json::json!(0.0));
    }

    #[test]
    fn arrays_and_objects_round_trip() {
        assert_round_trips(&serde_json::json!({
            "a": [1, 2, 3],
            "b": { "nested": true, "value": null },
            "c": "text",
        }));
    }
}
