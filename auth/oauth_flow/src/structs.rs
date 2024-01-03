use std::sync::{Arc, Mutex};

use core_entities::service::VersionedServiceTree;
use credential_entities::credentials::Authentication;

pub struct EnvironmentState {
    pub service: VersionedServiceTree,
    pub creds: Arc<Mutex<Authentication>>,
    pub redirect_uri: String,
}
