pub(crate) mod macos;

use crate::error::AppResult;
use crate::models::{InstallationInfo, ManagedState, PreflightReport};
use crate::state::InstallerPaths;
use async_trait::async_trait;
use std::path::PathBuf;

pub use macos::MacOsAdapter;

#[derive(Debug, Clone)]
pub struct PreflightContext {
    pub target: PathBuf,
    pub minimum_cmake_version: [u64; 3],
    pub operation_lock_held: bool,
}

#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    async fn discover_installations(
        &self,
        paths: &InstallerPaths,
        managed: &ManagedState,
    ) -> AppResult<Vec<InstallationInfo>>;

    async fn preflight(
        &self,
        paths: &InstallerPaths,
        context: &PreflightContext,
    ) -> AppResult<PreflightReport>;

    fn default_target(&self) -> AppResult<PathBuf>;
}
