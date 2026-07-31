use crate::error::{AppResult, InstallerError};
use crate::installer;
use crate::models::{
    InstallRequest, InstallationChannel, InstallationInfo, OperationProgress, OperationStage,
    PreflightReport, ReleaseInfo,
};
use crate::platform::{MacOsAdapter, PlatformAdapter};
use crate::releases;
use crate::state::AppState;
use std::path::{Path, PathBuf};
use tauri::ipc::Channel;
use tauri::State;
use url::Url;

#[tauri::command]
pub async fn list_releases(
    include_prereleases: bool,
    state: State<'_, AppState>,
) -> AppResult<Vec<ReleaseInfo>> {
    releases::list_releases(&state.client, &state.paths.cache_dir, include_prereleases).await
}

#[tauri::command]
pub async fn scan_installations(state: State<'_, AppState>) -> AppResult<Vec<InstallationInfo>> {
    let managed = state.load_managed_state()?;
    MacOsAdapter::new()
        .discover_installations(&state.paths, &managed)
        .await
}

#[tauri::command]
pub async fn run_preflight(state: State<'_, AppState>) -> AppResult<PreflightReport> {
    MacOsAdapter::new().preflight(&state.paths).await
}

#[tauri::command]
pub async fn install_build_tools(state: State<'_, AppState>) -> AppResult<PreflightReport> {
    let brew = crate::platform::macos::find_executable("brew").ok_or_else(|| {
        InstallerError::new(
            "homebrewMissing",
            "Homebrew is not installed. Install CMake and Ninja from their official websites.",
        )
    })?;
    let output = tokio::process::Command::new(&brew)
        .args(["install", "cmake", "ninja"])
        .env(
            "PATH",
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        )
        .output()
        .await?;
    if !output.status.success() {
        return Err(InstallerError::with_detail(
            "homebrew",
            "Homebrew could not install CMake and Ninja.",
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    MacOsAdapter::new().preflight(&state.paths).await
}

#[tauri::command]
pub async fn start_install(
    request: InstallRequest,
    progress: Channel<OperationProgress>,
    state: State<'_, AppState>,
) -> AppResult<InstallationInfo> {
    if !request.eula_accepted {
        return Err(InstallerError::new(
            "eulaRequired",
            "Accept the Aseprite EULA before compiling the source code.",
        ));
    }
    let cancelled = state.begin_operation()?;
    let result = async {
        let available = releases::list_releases(&state.client, &state.paths.cache_dir, true).await?;
        let release = available
            .iter()
            .find(|release| release.tag == request.tag)
            .ok_or_else(|| {
                InstallerError::new(
                    "unsupportedRelease",
                    "The selected Aseprite release is not supported or has no verified source archive.",
                )
            })?;
        let adapter = MacOsAdapter::new();
        let managed = state.load_managed_state()?;
        let installations = adapter
            .discover_installations(&state.paths, &managed)
            .await?;
        let default_target = adapter.default_target()?;
        let (target, existing) =
            resolve_target(&request, default_target, &installations)?;

        let preflight = adapter.preflight(&state.paths).await?;
        if !preflight.ready {
            return Err(InstallerError::new(
                "preflightFailed",
                "Install the missing build prerequisites before compiling Aseprite.",
            ));
        }
        installer::install_release(
            &state,
            release,
            &target,
            existing.as_ref(),
            cancelled,
            &progress,
        )
        .await
    }
    .await;
    state.finish_operation();
    if let Err(error) = &result {
        if error.code != "cancelled" {
            let _ = progress.send(OperationProgress::stage(
                OperationStage::Failed,
                None,
                error.message.clone(),
            ));
        }
    }
    result
}

fn resolve_target(
    request: &InstallRequest,
    default_target: PathBuf,
    installations: &[InstallationInfo],
) -> AppResult<(PathBuf, Option<InstallationInfo>)> {
    let Some(requested) = request.target_path.as_deref() else {
        let existing = find_by_path(installations, &default_target);
        if existing
            .as_ref()
            .is_some_and(|installation| installation.channel != InstallationChannel::Managed)
        {
            return Err(InstallerError::new(
                "adoptionRequired",
                "Adopt the existing manual copy before replacing it.",
            ));
        }
        return Ok((default_target, existing));
    };

    let requested_path = PathBuf::from(requested);
    let installation = find_by_path(installations, &requested_path).ok_or_else(|| {
        InstallerError::new(
            "unknownTarget",
            "The requested installation target was not detected on this Mac.",
        )
    })?;

    match installation.channel {
        InstallationChannel::Managed => Ok((requested_path, Some(installation))),
        InstallationChannel::Manual if request.adopt => {
            if installation.writable {
                Ok((requested_path, Some(installation)))
            } else {
                let default_existing = find_by_path(installations, &default_target);
                if default_existing.is_some() {
                    return Err(InstallerError::new(
                        "defaultOccupied",
                        "The managed ~/Applications destination already contains another Aseprite copy.",
                    ));
                }
                Ok((default_target, None))
            }
        }
        InstallationChannel::Manual => Err(InstallerError::new(
            "adoptionRequired",
            "Confirm adoption before replacing a manual Aseprite installation.",
        )),
        InstallationChannel::Steam | InstallationChannel::PackageManager => {
            Err(InstallerError::new(
                "externalChannel",
                "Steam and package-manager installations must be updated through their original channel.",
            ))
        }
    }
}

fn find_by_path(installations: &[InstallationInfo], path: &Path) -> Option<InstallationInfo> {
    let normalized = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    installations
        .iter()
        .find(|installation| {
            std::fs::canonicalize(&installation.path)
                .unwrap_or_else(|_| PathBuf::from(&installation.path))
                == normalized
        })
        .cloned()
}

async fn installation_by_id(state: &AppState, id: &str) -> AppResult<InstallationInfo> {
    let managed = state.load_managed_state()?;
    MacOsAdapter::new()
        .discover_installations(&state.paths, &managed)
        .await?
        .into_iter()
        .find(|installation| installation.id == id)
        .ok_or_else(|| InstallerError::new("notFound", "The Aseprite installation was not found."))
}

#[tauri::command]
pub fn cancel_operation(state: State<'_, AppState>) -> AppResult<()> {
    state.cancel_operation()
}

#[tauri::command]
pub async fn launch_installation(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let installation = installation_by_id(&state, &id).await?;
    run_open(&[installation.path.as_str()]).await
}

#[tauri::command]
pub async fn reveal_installation(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let installation = installation_by_id(&state, &id).await?;
    run_open(&["-R", installation.path.as_str()]).await
}

#[tauri::command]
pub async fn restore_previous(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<InstallationInfo> {
    let _cancelled = state.begin_operation()?;
    let result = installer::restore_previous(&state, &id).await;
    state.finish_operation();
    result
}

#[tauri::command]
pub fn uninstall_managed(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let _cancelled = state.begin_operation()?;
    let result = installer::uninstall_managed(&state, &id);
    state.finish_operation();
    result
}

#[tauri::command]
pub fn clean_cache(state: State<'_, AppState>) -> AppResult<u64> {
    let _cancelled = state.begin_operation()?;
    let result = installer::clean_cache(&state);
    state.finish_operation();
    result
}

#[tauri::command]
pub async fn open_external(url: String) -> AppResult<()> {
    let parsed = Url::parse(&url).map_err(|_| {
        InstallerError::new("externalUrl", "The requested external URL is invalid.")
    })?;
    let allowed = parsed.scheme() == "https"
        && match parsed.host_str() {
            Some("www.aseprite.org") | Some("aseprite.org") => true,
            Some("github.com") => {
                parsed.path().starts_with("/aseprite/aseprite")
                    || parsed.path().starts_with("/fmhun/asprite-installer")
            }
            _ => false,
        };
    if !allowed {
        return Err(InstallerError::new(
            "externalUrl",
            "Only official Aseprite and installer project links can be opened.",
        ));
    }
    run_open(&[url.as_str()]).await
}

async fn run_open(arguments: &[&str]) -> AppResult<()> {
    let output = tokio::process::Command::new("/usr/bin/open")
        .args(arguments)
        .output()
        .await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(InstallerError::with_detail(
            "open",
            "macOS could not open the requested item.",
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installation(path: &str, channel: InstallationChannel, writable: bool) -> InstallationInfo {
        InstallationInfo {
            id: "id".into(),
            path: path.into(),
            version: Some("1.3".into()),
            version_exact: false,
            architecture: None,
            channel,
            manageable: writable,
            writable,
            has_backup: false,
            installed_at: None,
        }
    }

    #[test]
    fn manual_target_requires_explicit_adoption() {
        let target = PathBuf::from("/Applications/Aseprite.app");
        let request = InstallRequest {
            tag: "v1.3.18.1".into(),
            target_path: Some(target.to_string_lossy().into_owned()),
            adopt: false,
            eula_accepted: true,
        };
        let result = resolve_target(
            &request,
            PathBuf::from("/Users/test/Applications/Aseprite.app"),
            &[installation(
                "/Applications/Aseprite.app",
                InstallationChannel::Manual,
                true,
            )],
        );
        assert_eq!(result.unwrap_err().code, "adoptionRequired");
    }
}
