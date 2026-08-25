//! Well-known file locations a [`Fetcher`](crate::Fetcher) is expected to
//! serve for a loaded service.

/// Location of a service's credentials file.
pub const CREDENTIALS_LOCATION: &str = "./credentials.json";

/// Location of a service's override configuration file.
pub const CONFIG_LOCATION: &str = "./config.json";

/// Location of a service's manifest file.
pub const MANIFEST_LOCATION: &str = "./manifest.json";
