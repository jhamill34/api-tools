//!

use serde::{Deserialize, Serialize};

///
#[derive(Serialize, Deserialize, Default)]
pub struct Configuration {
    ///
    pub oauth: OauthConfiguration,

    ///
    pub template: TemplateConfiguration,

    ///
    pub client: ClientConfiguration,
}

///
#[derive(Serialize, Deserialize, Default)]
pub struct ClientConfiguration {
    ///
    pub host: String,

    ///
    pub port: u16,
}

///
#[derive(Serialize, Deserialize, Default)]
pub struct OauthConfiguration {
    ///
    pub base_uri: String,

    ///
    pub cert_path: String,

    ///
    pub key_path: String,
}

///
#[derive(Serialize, Deserialize, Default)]
pub struct TemplateConfiguration {
    ///
    pub path: String,
}
