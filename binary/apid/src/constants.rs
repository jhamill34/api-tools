//! Shared constants for the daemon.

/// Environment variable naming the path to the daemon's TOML config file.
pub const CONFIG_PATH: &str = "APID_CONFIG_PATH";

/// The [`execution_engine::Engine`] language key registered for the Python
/// code runner.
pub const PYTHON_LANG: &str = "python";

/// The [`execution_engine::Engine`] language key registered for the
/// JavaScript code runner.
pub const JAVASCRIPT_LANG: &str = "js";
