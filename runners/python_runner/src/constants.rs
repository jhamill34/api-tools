//! Shared constants for the Python runner and its script bindings.

/// Key under which [`bindings::WorkflowLogger`](crate::bindings::WorkflowLogger)
/// stores a successful outcome in the output dict.
pub const RESPONSE_SUCCESS_KEY: &str = "success";

/// Key under which [`bindings::WorkflowLogger`](crate::bindings::WorkflowLogger)
/// stores a failure outcome in the output dict.
pub const RESPONSE_ERROR_KEY: &str = "error";

/// Key under which a custom output payload is stored within a
/// success/error outcome.
pub const RESPONSE_CUSTOM_KEY: &str = "custom";

/// Key under which a standard output payload is stored within a
/// success/error outcome.
pub const RESPONSE_STANDARD_KEY: &str = "standard";

/// Name the `api` binding is exposed under in a script's module namespace.
pub const BINDING_API_KEY: &str = "api";

/// Name the `action` binding is exposed under in a script's module
/// namespace.
pub const BINDING_ACTION_KEY: &str = "action";

/// Name the `workflow` binding is exposed under in a script's module
/// namespace.
pub const BINDING_WORKFLOW_KEY: &str = "workflow";

/// Name the `task` binding is exposed under in a script's module
/// namespace.
pub const BINDING_TASK_KEY: &str = "task";

/// Log-level tag used for error-level script log entries.
pub const LOG_ERROR: &str = "ERROR";

/// Log-level tag used for info-level script log entries.
pub const LOG_INFO: &str = "INFO";

/// Log-level tag used for warning-level script log entries.
pub const LOG_WARN: &str = "WARN";

/// Log-level tag used for success-level script log entries.
pub const LOG_SUCCESS: &str = "SUCCESS";

/// Log-level tag used for status-update script log entries.
pub const LOG_STATUS: &str = "STATUS";

/// The function name called in a script when none could be detected from
/// its source.
pub const DEFAULT_FUNCTION_NAME: &str = "execute";

/// `chrono` format string used to timestamp script log entries.
pub const DATETIME_FORMAT: &str = "%a %b %e %Y %I:%M:%S %p";
