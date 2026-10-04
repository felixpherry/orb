//! The services actors use, created once at startup and shared by cloning.

use crate::feat::git::git_service::GitService;
use crate::feat::harness::Harnesses;

/// Every service the actors depend on.
#[derive(Debug, Clone)]
pub struct Services {
    /// Every harness a thread can run in.
    pub harnesses: Harnesses,
    pub git: GitService,
}
