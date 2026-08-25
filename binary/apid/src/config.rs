//! The daemon's TOML configuration file, loaded from the path named by
//! [`crate::constants::CONFIG_PATH`].

use serde::{Deserialize, Serialize};

/// Top-level daemon configuration.
#[derive(Serialize, Deserialize)]
pub struct Configuration {
    /// Where to find loaded services' connector directories. Defaults to
    /// `~/connectors` when absent.
    pub connector: Option<ConnectorConfiguration>,

    /// Log file locations.
    pub log: LogConfiguration,

    /// gRPC server bind address.
    pub server: ServerConfiguration,
}

/// Configures where loaded services are read from.
#[derive(Serialize, Deserialize)]
pub struct ConnectorConfiguration {
    /// Directory containing one subdirectory per service.
    pub path: Option<String>,
}

/// Log file locations for the two kinds of runtime logging the engine
/// does.
#[derive(Serialize, Deserialize)]
pub struct LogConfiguration {
    /// Where the API-call connector logs each outbound HTTP call.
    pub api_path: String,

    /// Where the engine logs workflow-level execution activity.
    pub workflow_path: String,
}

/// The address the gRPC server listens on.
#[derive(Serialize, Deserialize)]
pub struct ServerConfiguration {
    /// TCP port to bind.
    pub port: u16,

    /// Host/interface to bind.
    pub host: String,
}
