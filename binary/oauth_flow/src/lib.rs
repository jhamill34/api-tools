pub mod error;
pub mod routes;
pub mod structs;

#[macro_use]
extern crate rocket;

use routes::{authorize, callback};
use structs::EnvironmentState;

use std::path::Path;
use std::sync::{Arc, Mutex};

use core_entities::service::VersionedServiceTree;
use credential_entities::credentials::Authentication;
use rocket::config::TlsConfig;

/// The `base_path`/`key_path`/`cert_path` fields all carry the "path"
/// suffix deliberately, not by accident: `base_path` is the redirect base
/// URI, while `key_path`/`cert_path` are filesystem paths to TLS
/// credentials - collapsing the names would blur that distinction.
#[allow(
    clippy::struct_field_names,
    reason = "each _path suffix distinguishes a URI base from filesystem paths to TLS files"
)]
pub struct Authenticator {
    base_path: String,
    key_path: String,
    cert_path: String,
}
impl Authenticator {
    #[must_use]
    pub fn new(base_path: String, key_path: String, cert_path: String) -> Self {
        Self {
            base_path,
            key_path,
            cert_path,
        }
    }

    /// Starts the embedded OAuth callback web server and blocks until the
    /// flow completes (or fails).
    ///
    /// # Errors
    /// Returns an error if reading the environment, parsing JSON, or
    /// running the embedded Rocket server fails.
    pub async fn start(
        &self,
        name: String,
        service: VersionedServiceTree,
        creds: Arc<Mutex<Authentication>>,
    ) -> error::Result<()> {
        println!("Waiting for Oauth flow to complete for {name}");
        println!("Please visit the following URL in your browser to start:");
        println!("    {}/oauth/authorize", self.base_path);

        let mut redirect_uri = self.base_path.clone();
        redirect_uri.push_str("/oauth/callback");

        let routes = routes![authorize::route, callback::route];

        let environment = EnvironmentState {
            redirect_uri,
            service,
            creds: Arc::clone(&creds),
        };

        let key = Path::new(&self.key_path);
        let certs = Path::new(&self.cert_path);

        let tls = TlsConfig::from_paths(certs, key);

        let figment = rocket::Config::figment()
            .merge(("tls", tls))
            .merge(("log_level", "off"));

        rocket::custom(figment)
            .mount("/", routes)
            .manage(environment)
            .launch()
            .await?;

        Ok(())
    }
}
