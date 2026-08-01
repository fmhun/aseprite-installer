use crate::error::{AppResult, InstallerError};
use crate::installer;
use crate::models::{
    InstallRequest, InstallationChannel, InstallationInfo, OperationProgress, OperationStage,
    PlatformInfo, PreflightReport, RecoveryStatus, ReleaseInfo,
};
use crate::platform::{current_adapter, PlatformAdapter, PreflightContext};
use crate::releases;
use crate::state::AppState;
#[cfg(target_os = "macos")]
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Stdio;
#[cfg(target_os = "macos")]
use std::sync::atomic::Ordering;
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::{LogicalSize, State, Window};
use url::Url;

fn validated_window_height(height: f64) -> AppResult<f64> {
    if height.is_finite() && (420.0..=2_000.0).contains(&height) {
        Ok(height.round())
    } else {
        Err(InstallerError::new(
            "invalidWindowHeight",
            "The requested installer window height is invalid.",
        ))
    }
}

#[tauri::command]
pub fn resize_window(height: f64, window: Window) -> AppResult<()> {
    let height = validated_window_height(height)?;
    let scale_factor = window.scale_factor().map_err(|error| {
        InstallerError::with_detail(
            "windowResize",
            "The installer window could not be resized.",
            error.to_string(),
        )
    })?;
    let inner_size = window.inner_size().map_err(|error| {
        InstallerError::with_detail(
            "windowResize",
            "The installer window could not be resized.",
            error.to_string(),
        )
    })?;
    let width = f64::from(inner_size.width) / scale_factor;
    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|error| {
            InstallerError::with_detail(
                "windowResize",
                "The installer window could not be resized.",
                error.to_string(),
            )
        })
}

#[tauri::command]
pub async fn list_releases(
    include_prereleases: bool,
    state: State<'_, AppState>,
) -> AppResult<Vec<ReleaseInfo>> {
    let client = state.http_client()?;
    releases::list_releases(&client, &state.paths.cache_dir, include_prereleases).await
}

#[tauri::command]
pub fn get_platform_info() -> AppResult<PlatformInfo> {
    crate::platform::current_platform_info()
}

#[tauri::command]
pub async fn scan_installations(state: State<'_, AppState>) -> AppResult<Vec<InstallationInfo>> {
    let _observation = state.begin_observation()?;
    let managed = state
        .load_managed_state()
        .map_err(|error| map_plan_storage_error(error, &state.paths.registry_file))?;
    current_adapter()
        .discover_installations(&state.paths, &managed)
        .await
}

#[tauri::command]
pub async fn run_preflight(
    tag: String,
    target_path: Option<String>,
    adopt: bool,
    state: State<'_, AppState>,
) -> AppResult<PreflightReport> {
    let _observation = state.begin_observation()?;
    let request = preflight_request(tag, target_path, adopt);
    let plan = resolve_install_plan(&state, &request).await?;
    let context = preflight_context_with_validated_operation_lock(&plan.preflight_context);
    current_adapter().preflight(&state.paths, &context).await
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn install_build_tools(
    tag: String,
    target_path: Option<String>,
    adopt: bool,
    state: State<'_, AppState>,
) -> AppResult<PreflightReport> {
    let cancelled = state.begin_operation()?;
    let result = async {
        let request = preflight_request(tag, target_path, adopt);
        let plan = resolve_install_plan(&state, &request).await?;
        let brew = crate::platform::macos::usable_homebrew_path().map_err(|detail| {
            InstallerError::with_detail(
                "homebrewUnavailable",
                "Homebrew is missing or cannot safely write to its own directories. Install CMake and Ninja manually, or repair Homebrew without sudo.",
                detail,
            )
        })?;
        run_homebrew_install(&state, &brew, &cancelled).await?;
        let context = preflight_context_with_validated_operation_lock(&plan.preflight_context);
        current_adapter().preflight(&state.paths, &context).await
    }
    .await;
    state.finish_operation();
    result
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn install_build_tools(
    _tag: String,
    _target_path: Option<String>,
    _adopt: bool,
    _state: State<'_, AppState>,
) -> AppResult<PreflightReport> {
    Err(InstallerError::new(
        "manualPrerequisites",
        "Build prerequisites must be installed explicitly using the platform guidance. Aseprite Installer never elevates itself or changes the system toolchain.",
    ))
}

#[cfg(target_os = "macos")]
async fn run_homebrew_install(
    state: &AppState,
    brew: &Path,
    cancelled: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> AppResult<()> {
    std::fs::create_dir_all(&state.paths.logs_dir)?;
    let log_path = state
        .paths
        .logs_dir
        .join(format!("homebrew-{}.log", uuid::Uuid::new_v4()));
    let stdout = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&log_path)?;
    let stderr = stdout.try_clone()?;
    let mut child = tokio::process::Command::new(brew)
        .args(["install", "cmake", "ninja"])
        .env("PATH", homebrew_command_path(brew))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            InstallerError::with_detail(
                "homebrewStart",
                "Homebrew could not be started.",
                error.to_string(),
            )
        })?;
    let process_id = child.id();
    let started = Instant::now();
    let status = loop {
        if cancelled.load(Ordering::SeqCst) {
            terminate_command_group(&mut child, process_id).await;
            let _ = std::fs::remove_file(&log_path);
            return Err(InstallerError::new(
                "cancelled",
                "The Homebrew installation was cancelled.",
            ));
        }
        if started.elapsed() > Duration::from_secs(30 * 60) {
            terminate_command_group(&mut child, process_id).await;
            return Err(InstallerError::with_detail(
                "homebrewTimeout",
                "Homebrew did not finish within 30 minutes and was stopped.",
                format!("Technical log: {}", log_path.display()),
            ));
        }
        if let Some(status) = child.try_wait()? {
            if let Some(process_id) = process_id {
                signal_command_group(process_id, "-KILL").await;
            }
            break status;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    };
    if status.success() {
        let _ = std::fs::remove_file(&log_path);
        return Ok(());
    }
    let output = std::fs::read_to_string(&log_path).unwrap_or_default();
    Err(InstallerError::with_detail(
        "homebrew",
        "Homebrew could not install CMake and Ninja. Do not retry it with sudo.",
        format!(
            "{}\nTechnical log: {}",
            tail_text(&output, 4_000),
            log_path.display()
        ),
    ))
}

#[cfg(target_os = "macos")]
async fn terminate_command_group(child: &mut tokio::process::Child, process_id: Option<u32>) {
    if let Some(process_id) = process_id {
        signal_command_group(process_id, "-TERM").await;
        for _ in 0..20 {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        signal_command_group(process_id, "-KILL").await;
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[cfg(target_os = "macos")]
async fn signal_command_group(process_id: u32, signal: &str) {
    let group = format!("-{process_id}");
    let _ = tokio::process::Command::new("/bin/kill")
        .args([signal, "--", &group])
        .status()
        .await;
}

#[cfg(target_os = "macos")]
fn tail_text(value: &str, maximum_characters: usize) -> String {
    let mut characters = value
        .chars()
        .rev()
        .take(maximum_characters)
        .collect::<Vec<_>>();
    characters.reverse();
    characters.into_iter().collect()
}

#[cfg(target_os = "macos")]
fn homebrew_command_path(brew: &Path) -> String {
    let parent = brew.parent().unwrap_or_else(|| Path::new("/usr/local/bin"));
    format!(
        "{}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        parent.display()
    )
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
        let plan = resolve_install_plan(&state, &request).await?;
        let adapter = current_adapter();
        let context = preflight_context_with_validated_operation_lock(&plan.preflight_context);
        let preflight = adapter.preflight(&state.paths, &context).await?;
        if !preflight.ready {
            return Err(InstallerError::new(
                "preflightFailed",
                "Install the missing build prerequisites before compiling Aseprite.",
            ));
        }
        installer::install_release(
            &state,
            &plan.release,
            &plan.target,
            plan.existing.as_ref(),
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

struct ResolvedInstallPlan {
    release: ReleaseInfo,
    target: PathBuf,
    existing: Option<InstallationInfo>,
    preflight_context: PreflightContext,
}

fn preflight_request(tag: String, target_path: Option<String>, adopt: bool) -> InstallRequest {
    InstallRequest {
        tag,
        target_path,
        adopt,
        eula_accepted: false,
    }
}

async fn resolve_install_plan(
    state: &AppState,
    request: &InstallRequest,
) -> AppResult<ResolvedInstallPlan> {
    let client = state.http_client()?;
    let available = releases::list_releases(&client, &state.paths.cache_dir, true).await?;
    let release = available
        .into_iter()
        .find(|release| release.tag == request.tag)
        .ok_or_else(unsupported_release_error)?;
    let adapter = current_adapter();
    let managed = state
        .load_managed_state()
        .map_err(|error| map_plan_storage_error(error, &state.paths.registry_file))?;
    let installations = adapter
        .discover_installations(&state.paths, &managed)
        .await?;
    let default_target = adapter.default_target()?;
    let (target, existing) = resolve_target(request, default_target, &installations)?;
    let preflight_context = build_preflight_context(&release, target.clone())?;

    Ok(ResolvedInstallPlan {
        release,
        target,
        existing,
        preflight_context,
    })
}

fn map_plan_storage_error(error: InstallerError, registry_file: &Path) -> InstallerError {
    if error.code != "io" {
        return error;
    }
    let detail = error
        .detail
        .unwrap_or_else(|| "The registry could not be read.".into());
    InstallerError::with_detail(
        "workspaceStorage",
        "The installer state could not be read before checking requirements. Restore read/write access to the installer data folder and do not run the installer with sudo.",
        format!("{}: {detail}", registry_file.display()),
    )
}

fn build_preflight_context(release: &ReleaseInfo, target: PathBuf) -> AppResult<PreflightContext> {
    let requirements = releases::source_build_requirements(&release.source_asset_name)
        .ok_or_else(unsupported_release_error)?;
    Ok(PreflightContext {
        target,
        minimum_cmake_version: requirements.minimum_cmake_version,
        operation_lock_held: false,
    })
}

fn preflight_context_with_validated_operation_lock(context: &PreflightContext) -> PreflightContext {
    let mut context = context.clone();
    // Commands call this only while holding either the shared observation lock
    // or the exclusive mutation lock. Platform probes must not try to acquire
    // the same operation lock exclusively and deadlock against their own
    // reader; the held lock already proves that no mutation can overlap.
    context.operation_lock_held = true;
    context
}

fn unsupported_release_error() -> InstallerError {
    InstallerError::new(
        "unsupportedRelease",
        "The selected Aseprite release is not supported or has no verified source archive.",
    )
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
        ensure_unclaimed_target_is_empty(&default_target, existing.as_ref())?;
        return Ok((default_target, existing));
    };

    let requested_path = PathBuf::from(requested);
    let installation = find_by_path(installations, &requested_path).ok_or_else(|| {
        InstallerError::new(
            "unknownTarget",
            "The requested installation target was not detected on this computer.",
        )
    })?;

    // Never carry the webview-provided spelling of a detected path into a
    // destructive operation. `find_by_path` may accept an equivalent lexical,
    // canonical, or case-insensitive spelling; the backend must then use the
    // freshly discovered installation path as the authoritative target.
    let detected_path = PathBuf::from(&installation.path);
    match installation.channel {
        InstallationChannel::Managed => Ok((detected_path, Some(installation))),
        InstallationChannel::Manual if request.adopt => {
            if installation.writable {
                Ok((detected_path, Some(installation)))
            } else {
                let default_existing = find_by_path(installations, &default_target);
                if default_existing.is_some() {
                    return Err(InstallerError::new(
                        "defaultOccupied",
                        "The managed destination already contains another Aseprite copy.",
                    ));
                }
                ensure_unclaimed_target_is_empty(&default_target, None)?;
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

fn ensure_unclaimed_target_is_empty(
    target: &Path,
    detected: Option<&InstallationInfo>,
) -> AppResult<()> {
    if detected.is_some() {
        return Ok(());
    }
    match std::fs::symlink_metadata(target) {
        Ok(_) => Err(InstallerError::new(
            "defaultOccupied",
            "The managed destination is occupied by an item that is not a valid detected Aseprite installation.",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(InstallerError::with_detail(
            "targetInspect",
            "The managed destination could not be inspected safely.",
            error.to_string(),
        )),
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
    current_adapter()
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
    let _observation = state.begin_observation()?;
    let installation = installation_by_id(&state, &id).await?;
    crate::platform::launch_path(Path::new(&installation.path)).await
}

#[tauri::command]
pub async fn reveal_installation(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let _observation = state.begin_observation()?;
    let installation = installation_by_id(&state, &id).await?;
    crate::platform::reveal_path(Path::new(&installation.path)).await
}

#[cfg(target_os = "macos")]
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

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[tauri::command]
pub async fn restore_previous(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<InstallationInfo> {
    let state = state.inner().clone();
    let _cancelled = state.begin_operation()?;
    let worker_state = state.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        installer::restore_previous(&worker_state, &id)
    })
    .await
    .map_err(|error| {
        InstallerError::with_detail(
            "operationWorker",
            "The restore worker stopped unexpectedly.",
            error.to_string(),
        )
    });
    state.finish_operation();
    result?
}

#[tauri::command]
pub async fn uninstall_managed(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let state = state.inner().clone();
    let _cancelled = state.begin_operation()?;
    let worker_state = state.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        installer::uninstall_managed(&worker_state, &id)
    })
    .await
    .map_err(|error| {
        InstallerError::with_detail(
            "operationWorker",
            "The uninstall worker stopped unexpectedly.",
            error.to_string(),
        )
    });
    state.finish_operation();
    result?
}

#[tauri::command]
pub async fn clean_cache(state: State<'_, AppState>) -> AppResult<u64> {
    let state = state.inner().clone();
    let _cancelled = state.begin_operation()?;
    let worker_state = state.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || installer::clean_cache(&worker_state))
            .await
            .map_err(|error| {
                InstallerError::with_detail(
                    "operationWorker",
                    "The cache-cleanup worker stopped unexpectedly.",
                    error.to_string(),
                )
            });
    state.finish_operation();
    result?
}

#[tauri::command]
pub fn get_recovery_status(state: State<'_, AppState>) -> RecoveryStatus {
    state.recovery_status()
}

#[tauri::command]
pub async fn retry_recovery(state: State<'_, AppState>) -> AppResult<RecoveryStatus> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || match state.begin_operation() {
        Ok(_) => {
            state.finish_operation();
            Ok(state.recovery_status())
        }
        Err(error) => Err(error),
    })
    .await
    .map_err(|error| {
        InstallerError::with_detail(
            "operationWorker",
            "The recovery worker stopped unexpectedly.",
            error.to_string(),
        )
    })?
}

#[tauri::command]
pub async fn open_external(url: String) -> AppResult<()> {
    let parsed = Url::parse(&url).map_err(|_| {
        InstallerError::new("externalUrl", "The requested external URL is invalid.")
    })?;
    if !is_allowed_external_url(&parsed) {
        return Err(InstallerError::new(
            "externalUrl",
            "Only approved official project and requirements documentation can be opened.",
        ));
    }
    crate::platform::open_external_url(url.as_str()).await
}

fn is_allowed_external_url(url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }

    match url.host_str() {
        Some("www.aseprite.org") | Some("aseprite.org") => true,
        Some("github.com") => {
            url.path().starts_with("/aseprite/aseprite")
                || url.path().starts_with("/fmhun/aseprite-installer")
                || url.path().starts_with("/ninja-build/ninja/releases")
        }
        Some("developer.apple.com") => url
            .path()
            .starts_with("/documentation/xcode/installing-the-command-line-tools"),
        Some("support.apple.com") => {
            url.path().starts_with("/en-us/108382")
                || url.path().starts_with("/en-us/102624")
                || url
                    .path()
                    .starts_with("/guide/mac-help/change-proxy-settings-on-mac-mchlp2591/mac")
                || url
                    .path()
                    .starts_with("/guide/disk-utility/file-system-formats-")
        }
        Some("cmake.org") => url.path().starts_with("/download"),
        Some("formulae.brew.sh") => {
            url.path() == "/formula/cmake" || url.path() == "/formula/ninja"
        }
        Some("learn.microsoft.com")
        | Some("developer.microsoft.com")
        | Some("support.microsoft.com")
        | Some("visualstudio.microsoft.com")
        | Some("gitforwindows.org")
        | Some("ubuntu.com")
        | Some("docs.fedoraproject.org")
        | Some("wiki.archlinux.org")
        | Some("doc.opensuse.org") => true,
        _ => false,
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

    #[test]
    fn detected_target_path_is_authoritative_after_equivalent_path_matching() {
        let directory = tempfile::tempdir().unwrap();
        let detected = directory.path().join("Aseprite");
        let intermediate = directory.path().join("intermediate");
        std::fs::create_dir(&detected).unwrap();
        std::fs::create_dir(&intermediate).unwrap();
        let requested = intermediate.join("..").join("Aseprite");
        let request = InstallRequest {
            tag: "v1.3.18.1".into(),
            target_path: Some(requested.to_string_lossy().into_owned()),
            adopt: false,
            eula_accepted: true,
        };

        let (resolved, _) = resolve_target(
            &request,
            directory.path().join("default"),
            &[installation(
                &detected.to_string_lossy(),
                InstallationChannel::Managed,
                true,
            )],
        )
        .unwrap();

        assert_eq!(resolved, detected);
        assert_ne!(resolved, requested);
    }

    #[test]
    fn refuses_to_overwrite_an_unrecognized_default_target() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("Aseprite.app");
        std::fs::write(&target, b"not an application bundle").unwrap();
        let request = InstallRequest {
            tag: "v1.3.18.1".into(),
            target_path: None,
            adopt: false,
            eula_accepted: true,
        };

        let error = resolve_target(&request, target, &[]).unwrap_err();
        assert_eq!(error.code, "defaultOccupied");
    }

    #[test]
    fn preflight_context_uses_the_source_asset_version() {
        let release = ReleaseInfo {
            tag: "v1.3.15.4".into(),
            name: "Aseprite v1.3.15.4".into(),
            published_at: String::new(),
            prerelease: false,
            latest: false,
            source_asset_name: "Aseprite-v1.3.15.5-Source.zip".into(),
            source_url: "https://github.com/aseprite/aseprite/releases/download/v1.3.15.4/Aseprite-v1.3.15.5-Source.zip".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            size: 1,
        };
        let target = PathBuf::from("/Users/test/Applications/Aseprite.app");

        let context = build_preflight_context(&release, target.clone()).unwrap();

        assert_eq!(context.target, target);
        assert_eq!(context.minimum_cmake_version, [3, 20, 0]);
    }

    #[test]
    fn locked_preflight_context_skips_reacquiring_the_operation_lock() {
        let original = PreflightContext {
            target: PathBuf::from("/Users/test/Applications/Aseprite.app"),
            minimum_cmake_version: [3, 20, 0],
            operation_lock_held: false,
        };

        let locked = preflight_context_with_validated_operation_lock(&original);

        assert!(locked.operation_lock_held);
        assert!(!original.operation_lock_held);
        assert_eq!(locked.target, original.target);
        assert_eq!(locked.minimum_cmake_version, original.minimum_cmake_version);
    }

    #[test]
    fn preflight_context_rejects_an_invalid_source_asset() {
        let release = ReleaseInfo {
            tag: "v1.3.18.1".into(),
            name: "Aseprite v1.3.18.1".into(),
            published_at: String::new(),
            prerelease: false,
            latest: true,
            source_asset_name: "Aseprite-v1.2.40-Source.zip".into(),
            source_url: "https://github.com/aseprite/aseprite/releases/download/v1.3.18.1/Aseprite-v1.2.40-Source.zip".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            size: 1,
        };

        assert_eq!(
            build_preflight_context(
                &release,
                PathBuf::from("/Users/test/Applications/Aseprite.app")
            )
            .unwrap_err()
            .code,
            "unsupportedRelease"
        );
    }

    #[test]
    fn state_io_errors_keep_an_actionable_workspace_code() {
        let path = Path::new("/Users/test/Library/Application Support/state.json");
        let error = map_plan_storage_error(
            InstallerError::with_detail("io", "A file operation failed.", "Permission denied"),
            path,
        );

        assert_eq!(error.code, "workspaceStorage");
        assert!(error.message.contains("read/write access"));
        assert!(error.detail.as_deref().unwrap().contains("state.json"));
    }

    #[test]
    fn validates_window_height_bounds() {
        assert_eq!(validated_window_height(492.4).unwrap(), 492.0);
        assert_eq!(
            validated_window_height(419.0).unwrap_err().code,
            "invalidWindowHeight"
        );
        assert_eq!(
            validated_window_height(f64::NAN).unwrap_err().code,
            "invalidWindowHeight"
        );
    }

    #[test]
    fn restricts_external_links_to_approved_documentation() {
        for url in [
            "https://www.aseprite.org/buy/",
            "https://github.com/aseprite/aseprite/blob/main/INSTALL.md",
            "https://github.com/fmhun/aseprite-installer/issues/new/choose",
            "https://github.com/ninja-build/ninja/releases",
            "https://developer.apple.com/documentation/xcode/installing-the-command-line-tools",
            "https://support.apple.com/en-us/102624",
            "https://support.apple.com/guide/mac-help/change-proxy-settings-on-mac-mchlp2591/mac",
            "https://cmake.org/download/",
            "https://formulae.brew.sh/formula/cmake",
        ] {
            assert!(is_allowed_external_url(&Url::parse(url).unwrap()), "{url}");
        }

        for url in [
            "http://cmake.org/download/",
            "https://github.com/unapproved/project",
            "https://support.apple.com/en-us/unapproved",
            "https://example.com/",
        ] {
            assert!(!is_allowed_external_url(&Url::parse(url).unwrap()), "{url}");
        }
    }
}
