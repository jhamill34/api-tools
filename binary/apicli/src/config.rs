//! The CLI's TOML configuration file, loaded from the path named by
//! [`crate::constants::APICLI_CONFIG_PATH`].

use serde::{Deserialize, Serialize};

/// Top-level CLI configuration.
#[derive(Serialize, Deserialize, Default)]
pub struct Configuration {
    /// Interactive-login (OAuth) settings.
    pub oauth: OauthConfiguration,

    /// Service-generation template settings.
    pub template: TemplateConfiguration,

    /// The `apid` daemon this CLI talks to.
    pub client: ClientConfiguration,
}

/// The gRPC daemon (`apid`) this CLI connects to.
#[derive(Serialize, Deserialize, Default)]
pub struct ClientConfiguration {
    /// Daemon host/interface.
    pub host: String,

    /// Daemon port.
    pub port: u16,
}

/// Settings for the embedded interactive-login web server.
#[derive(Serialize, Deserialize, Default)]
pub struct OauthConfiguration {
    /// Base URI the local callback server listens on.
    pub base_uri: String,

    /// Path to the TLS certificate used by the local callback server.
    pub cert_path: String,

    /// Path to the TLS private key used by the local callback server.
    pub key_path: String,
}

/// Settings for the `generate` command's scaffolding templates.
#[derive(Serialize, Deserialize, Default)]
pub struct TemplateConfiguration {
    /// Directory containing the scaffolding templates.
    pub path: String,
}
