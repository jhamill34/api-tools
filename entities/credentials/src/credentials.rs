//! A service's credentials. JSON shape matches protobuf3's canonical JSON
//! mapping (the format this crate's types used to be generated from) byte
//! for byte, since `credentials.json` is an on-disk, user-authored file
//! (`service_loader::loaders::load_credentials`) and must keep parsing
//! unchanged.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A service's credentials, in whichever of the supported auth shapes it
/// uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Authentication {
    /// Username/password basic auth.
    Basic(BasicCredentials),

    /// A single static value sent as a header.
    Header(HeaderCredentials),

    /// A single static value sent as a query parameter.
    Query(QueryCredentials),

    /// A single static value substituted into the request path.
    Path(PathCredentials),

    /// OAuth 2.0 client credentials.
    Oauth(OAuthCredentials),

    /// Multiple static values, each sent as its own header.
    MultiHeader(MultiHeaderCredentials),
}

impl Authentication {
    /// Returns the basic-auth credentials, if that's this value's shape.
    #[must_use]
    pub fn as_basic(&self) -> Option<&BasicCredentials> {
        match self {
            Self::Basic(v) => Some(v),
            Self::Header(_) | Self::Query(_) | Self::Path(_) | Self::Oauth(_) | Self::MultiHeader(_) => None,
        }
    }

    /// Returns the header credentials, if that's this value's shape.
    #[must_use]
    pub fn as_header(&self) -> Option<&HeaderCredentials> {
        match self {
            Self::Header(v) => Some(v),
            Self::Basic(_) | Self::Query(_) | Self::Path(_) | Self::Oauth(_) | Self::MultiHeader(_) => None,
        }
    }

    /// Returns the query credentials, if that's this value's shape.
    #[must_use]
    pub fn as_query(&self) -> Option<&QueryCredentials> {
        match self {
            Self::Query(v) => Some(v),
            Self::Basic(_) | Self::Header(_) | Self::Path(_) | Self::Oauth(_) | Self::MultiHeader(_) => None,
        }
    }

    /// Returns the path credentials, if that's this value's shape.
    #[must_use]
    pub fn as_path(&self) -> Option<&PathCredentials> {
        match self {
            Self::Path(v) => Some(v),
            Self::Basic(_) | Self::Header(_) | Self::Query(_) | Self::Oauth(_) | Self::MultiHeader(_) => None,
        }
    }

    /// Returns the OAuth credentials, if that's this value's shape.
    #[must_use]
    pub fn as_oauth(&self) -> Option<&OAuthCredentials> {
        match self {
            Self::Oauth(v) => Some(v),
            Self::Basic(_) | Self::Header(_) | Self::Query(_) | Self::Path(_) | Self::MultiHeader(_) => None,
        }
    }

    /// Returns the multi-header credentials, if that's this value's shape.
    #[must_use]
    pub fn as_multi_header(&self) -> Option<&MultiHeaderCredentials> {
        match self {
            Self::MultiHeader(v) => Some(v),
            Self::Basic(_) | Self::Header(_) | Self::Query(_) | Self::Path(_) | Self::Oauth(_) => None,
        }
    }
}

/// Username/password basic auth credentials.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BasicCredentials {
    /// The username.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,

    /// The password.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
}

/// A single static value sent as a header.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderCredentials {
    /// The header's value.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
}

/// A single static value sent as a query parameter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryCredentials {
    /// The query parameter's value.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
}

/// A single static value substituted into the request path.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathCredentials {
    /// The path segment's value.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
}

/// OAuth 2.0 client credentials.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthCredentials {
    /// The OAuth client ID.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_id: String,

    /// The OAuth client secret.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_secret: String,

    /// The current access token, if one has already been obtained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
}

/// Multiple static values, each sent as its own header.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultiHeaderCredentials {
    /// Header name to header value.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub values: HashMap<String, String>,
}
