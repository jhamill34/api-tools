use std::sync::{Arc, Mutex};

use credential_entities::credentials::Authentication;
use core_entities::service::VersionedServiceTree;

pub struct EnvironmentState {
    pub service: VersionedServiceTree,
    pub creds: Arc<Mutex<Authentication>>,
    pub redirect_uri: String,
}
