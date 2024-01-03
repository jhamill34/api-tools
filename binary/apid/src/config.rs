//!

use serde::{Deserialize, Serialize};

///
#[derive(Serialize, Deserialize)]
pub struct Configuration {
    ///
    pub connector: Option<ConnectorConfiguration>,

    ///
    pub log: LogConfiguration,

    ///
    pub server: ServerConfiguration,
}

///
#[derive(Serialize, Deserialize)]
pub struct ConnectorConfiguration {
    ///
    pub path: Option<String>,
}

///
#[derive(Serialize, Deserialize)]
pub struct LogConfiguration {
    ///
    pub api_path: String,

    ///
    pub workflow_path: String,
}

///
#[derive(Serialize, Deserialize)]
pub struct ServerConfiguration {
    ///
    pub port: u16,

    ///
    pub host: String,
}
