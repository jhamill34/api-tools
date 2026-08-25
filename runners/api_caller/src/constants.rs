//! Shared constants for the API-call runner.

/// `chrono` format string used to timestamp request/response log entries.
pub const DATETIME_FORMAT: &str = "%a %b %e %Y %I:%M:%S %p";

/// The runtime-expression prefix a `resultsPath`/cursor path is stripped of
/// before being parsed as a JSON pointer into the response body.
pub const RESPONSE_BODY_PREFIX: &str = "$response.body#";

/// The result-count cap used when a run's `options` don't specify a
/// `limit`. `0` means unpaginated: only the first page is fetched.
pub const DEFAULT_LIMIT: i32 = 0;
