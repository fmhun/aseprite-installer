#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "windows")]
pub(crate) mod windows;

use crate::error::AppResult;
use crate::models::{InstallationInfo, ManagedState, PlatformId, PlatformInfo, PreflightReport};
use crate::state::InstallerPaths;
use async_trait::async_trait;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
pub use linux::LinuxAdapter as CurrentAdapter;
#[cfg(target_os = "macos")]
pub use macos::MacOsAdapter as CurrentAdapter;
#[cfg(target_os = "windows")]
pub use windows::WindowsAdapter as CurrentAdapter;

pub fn current_adapter() -> CurrentAdapter {
    CurrentAdapter::new()
}

pub fn current_platform_info() -> AppResult<PlatformInfo> {
    let target = current_adapter().default_target()?;
    let architecture = std::env::consts::ARCH.to_owned();

    #[cfg(target_os = "macos")]
    let (id, display_name, file_manager_name, trash_name, shell_name, supported) = (
        PlatformId::Macos,
        "macOS",
        "Finder",
        "Trash",
        "Terminal",
        matches!(architecture.as_str(), "aarch64" | "x86_64"),
    );
    #[cfg(target_os = "windows")]
    let (id, display_name, file_manager_name, trash_name, shell_name, supported) = (
        PlatformId::Windows,
        "Windows",
        "File Explorer",
        "Recycle Bin",
        "PowerShell",
        architecture == "x86_64",
    );
    #[cfg(target_os = "linux")]
    let (id, display_name, file_manager_name, trash_name, shell_name, supported) = (
        PlatformId::Linux,
        "Linux",
        "Files",
        "Trash",
        "Terminal",
        architecture == "x86_64",
    );

    Ok(PlatformInfo {
        id,
        display_name: display_name.into(),
        architecture: architecture.clone(),
        supported,
        unsupported_reason: (!supported).then(|| {
            format!(
                "Aseprite Installer does not publish a supported build path for {architecture} on {display_name}."
            )
        }),
        default_target_path: target.to_string_lossy().into_owned(),
        file_manager_name: file_manager_name.into(),
        trash_name: trash_name.into(),
        shell_name: shell_name.into(),
    })
}

pub async fn launch_path(path: &std::path::Path) -> AppResult<()> {
    #[cfg(target_os = "macos")]
    {
        let output = tokio::process::Command::new("/usr/bin/open")
            .arg(path)
            .output()
            .await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(crate::error::InstallerError::with_detail(
                "launch",
                "Aseprite could not be launched.",
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ))
        }
    }
    #[cfg(target_os = "linux")]
    {
        linux::launch(path).await
    }
    #[cfg(target_os = "windows")]
    {
        windows::launch(path).await
    }
}

pub async fn reveal_path(path: &std::path::Path) -> AppResult<()> {
    #[cfg(target_os = "macos")]
    {
        let output = tokio::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(path)
            .output()
            .await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(crate::error::InstallerError::with_detail(
                "reveal",
                "The installation could not be shown in the file manager.",
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ))
        }
    }
    #[cfg(target_os = "linux")]
    {
        linux::reveal(path).await
    }
    #[cfg(target_os = "windows")]
    {
        windows::reveal(path).await
    }
}

pub async fn open_external_url(url: &str) -> AppResult<()> {
    #[cfg(target_os = "macos")]
    {
        let mut command = tokio::process::Command::new("/usr/bin/open");
        command.arg(url);
        let output = command.output().await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(crate::error::InstallerError::with_detail(
                "open",
                "The requested item could not be opened.",
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ))
        }
    }
    #[cfg(target_os = "linux")]
    {
        linux::open_external(url).await
    }
    #[cfg(target_os = "windows")]
    {
        windows::open_external(url).await
    }
}

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
