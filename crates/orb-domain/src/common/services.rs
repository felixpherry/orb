//! The services actors use, created once at startup and shared by cloning.

use crate::feat::sessions::session_host::SessionHostService;

/// Every service the actors depend on.
#[derive(Debug, Clone)]
pub struct Services {
    pub session_host: SessionHostService,
}
