pub mod authorize;
pub mod callback;

use crate::error::CallbackError;

/// Errors with `message` if `value` is empty, otherwise returns it back -
/// the shared body of every "this connector config/credential field must be
/// set" check in [`authorize`] and [`callback`].
pub(super) fn require_non_empty<'value>(
    value: &'value str,
    message: &'static str,
) -> Result<&'value str, CallbackError> {
    if value.is_empty() {
        Err(CallbackError::MissingConfig(message))
    } else {
        Ok(value)
    }
}

/// `value` if it's non-empty, otherwise `default` - for connector config
/// fields that fall back to a default instead of erroring when unset.
pub(super) fn or_default<'value>(value: &'value str, default: &'value str) -> &'value str {
    if value.is_empty() {
        default
    } else {
        value
    }
}
