use crate::error::{AppResult, InstallerError};
use crate::models::{
    InstallationInfo, ManagedRecord, OperationProgress, OperationStage, ReleaseInfo,
};
use crate::platform::macos::{installation_id, is_aseprite_bundle};
use crate::state::AppState;
use chrono::Utc;
use futures_util::StreamExt;
use plist::Value;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::ipc::Channel;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use uuid::Uuid;
use walkdir::WalkDir;

const LOCAL_ASEPRITE_ICON_NAME: &str = "AsepriteInstallerLocal.icns";
const LOCAL_ASEPRITE_ICON: &[u8] = include_bytes!("../resources/aseprite-local.icns");

pub async fn install_release(
    state: &AppState,
    release: &ReleaseInfo,
    target: &Path,
    existing: Option<&InstallationInfo>,
    cancelled: Arc<AtomicBool>,
    progress: &Channel<OperationProgress>,
) -> AppResult<InstallationInfo> {
    send_stage(
        progress,
        OperationStage::Preflight,
        Some(1),
        "Checking your Mac…",
    );
    ensure_not_cancelled(&cancelled)?;
    ensure_target_is_safe(target)?;
    ensure_aseprite_is_closed()?;

    let operation_id = Uuid::new_v4().to_string();
    let work_dir = state
        .paths
        .builds_dir
        .join(format!("{}-{operation_id}", release.tag));
    std::fs::create_dir_all(&work_dir)?;
    let log_path = state.paths.logs_dir.join(format!(
        "{}-{}.log",
        Utc::now().format("%Y%m%d-%H%M%S"),
        release.tag
    ));
    let mut log_file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&log_path)?;

    let result = async {
        let archive = download_archive(state, release, &cancelled, progress).await?;
        ensure_not_cancelled(&cancelled)?;

        send_stage(
            progress,
            OperationStage::Extracting,
            Some(30),
            "Extracting the verified source archive…",
        );
        let archive_for_extract = archive.clone();
        let work_for_extract = work_dir.clone();
        tauri::async_runtime::spawn_blocking(move || {
            extract_archive_safely(&archive_for_extract, &work_for_extract)
        })
        .await
        .map_err(|error| {
            InstallerError::with_detail(
                "extract",
                "The source archive could not be extracted.",
                error.to_string(),
            )
        })??;
        ensure_not_cancelled(&cancelled)?;

        let source_root = find_source_root(&work_dir)?;
        make_build_script_executable(&source_root.join("build.sh"))?;
        send_stage(
            progress,
            OperationStage::Compiling,
            None,
            "Compiling Aseprite from official sources…",
        );
        run_build(&source_root, &cancelled, progress, &mut log_file).await?;
        ensure_not_cancelled(&cancelled)?;

        let built_app = source_root.join("build/bin/Aseprite.app");
        if !is_aseprite_bundle(&built_app) {
            return Err(InstallerError::new(
                "invalidBuild",
                "The build completed without producing a valid Aseprite.app bundle.",
            ));
        }

        apply_local_aseprite_icon(&built_app)?;

        send_stage(
            progress,
            OperationStage::Signing,
            Some(78),
            "Applying the local app icon and ad-hoc signature…",
        );
        run_checked(
            "/usr/bin/codesign",
            &[
                "--force",
                "--deep",
                "--sign",
                "-",
                built_app.to_string_lossy().as_ref(),
            ],
            "Aseprite could not be signed locally.",
        )
        .await?;
        run_checked(
            "/usr/bin/codesign",
            &[
                "--verify",
                "--deep",
                "--strict",
                built_app.to_string_lossy().as_ref(),
            ],
            "The locally signed Aseprite bundle did not pass validation.",
        )
        .await?;

        install_atomically(state, release, target, &built_app, existing, progress).await
    }
    .await;

    match &result {
        Ok(_) => {
            let _ = std::fs::remove_dir_all(&work_dir);
            prune_files(&state.paths.archives_dir, 3);
            prune_files(&state.paths.logs_dir, 10);
        }
        Err(error) if error.code == "cancelled" => {
            let _ = std::fs::remove_dir_all(&work_dir);
            let _ = progress.send(OperationProgress::stage(
                OperationStage::Cancelled,
                None,
                "Operation cancelled. The active installation was not changed.",
            ));
        }
        Err(_) => {}
    }

    result
}

fn apply_local_aseprite_icon(app_bundle: &Path) -> AppResult<()> {
    if !is_aseprite_bundle(app_bundle) {
        return Err(InstallerError::new(
            "invalidBundle",
            "The local Aseprite icon can only be applied to a valid Aseprite bundle.",
        ));
    }

    let contents = app_bundle.join("Contents");
    let resources = contents.join("Resources");
    let info_path = contents.join("Info.plist");
    let temporary_info_path = contents.join(".Info.plist.aseprite-installer");
    std::fs::create_dir_all(&resources)?;
    std::fs::write(
        resources.join(LOCAL_ASEPRITE_ICON_NAME),
        LOCAL_ASEPRITE_ICON,
    )?;

    let mut info = Value::from_file(&info_path).map_err(|error| {
        InstallerError::with_detail(
            "bundleMetadata",
            "The built Aseprite metadata could not be read.",
            error.to_string(),
        )
    })?;
    let dictionary = info.as_dictionary_mut().ok_or_else(|| {
        InstallerError::new(
            "bundleMetadata",
            "The built Aseprite metadata has an invalid format.",
        )
    })?;
    dictionary.insert(
        "CFBundleIconFile".into(),
        Value::String(LOCAL_ASEPRITE_ICON_NAME.into()),
    );
    info.to_file_xml(&temporary_info_path).map_err(|error| {
        InstallerError::with_detail(
            "bundleMetadata",
            "The local Aseprite icon could not be registered.",
            error.to_string(),
        )
    })?;
    std::fs::rename(temporary_info_path, info_path)?;
    Ok(())
}

async fn download_archive(
    state: &AppState,
    release: &ReleaseInfo,
    cancelled: &AtomicBool,
    progress: &Channel<OperationProgress>,
) -> AppResult<PathBuf> {
    validate_asset_name(&release.source_asset_name)?;
    let archive_path = state.paths.archives_dir.join(&release.source_asset_name);
    if archive_path.exists() {
        send_stage(
            progress,
            OperationStage::Verifying,
            Some(8),
            "Checking the cached source archive…",
        );
        if verify_sha256(&archive_path, &release.digest)? {
            return Ok(archive_path);
        }
        std::fs::remove_file(&archive_path)?;
    }

    send_stage(
        progress,
        OperationStage::Downloading,
        Some(5),
        "Downloading the official Aseprite source archive…",
    );
    let partial = archive_path.with_extension("zip.part");
    if partial.exists() {
        std::fs::remove_file(&partial)?;
    }
    let response = state
        .client
        .get(&release.source_url)
        .send()
        .await?
        .error_for_status()?;
    let total = response
        .content_length()
        .or(Some(release.size))
        .filter(|size| *size > 0);
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&partial).await?;
    let mut downloaded = 0_u64;
    while let Some(chunk) = stream.next().await {
        ensure_not_cancelled(cancelled)?;
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        let percent = total.map(|total| {
            let download_percent = ((downloaded as f64 / total as f64) * 20.0) as u8;
            5_u8.saturating_add(download_percent.min(20))
        });
        send_stage(
            progress,
            OperationStage::Downloading,
            percent,
            "Downloading the official Aseprite source archive…",
        );
    }
    file.flush().await?;
    drop(file);

    send_stage(
        progress,
        OperationStage::Verifying,
        Some(27),
        "Verifying the GitHub SHA-256 digest…",
    );
    if !verify_sha256(&partial, &release.digest)? {
        let _ = std::fs::remove_file(&partial);
        return Err(InstallerError::new(
            "checksumMismatch",
            "The downloaded archive did not match GitHub’s SHA-256 digest.",
        ));
    }
    std::fs::rename(&partial, &archive_path)?;
    Ok(archive_path)
}

fn validate_asset_name(name: &str) -> AppResult<()> {
    if name.contains('/')
        || name.contains('\\')
        || !name.starts_with("Aseprite-v1.3")
        || !name.ends_with("-Source.zip")
    {
        return Err(InstallerError::new(
            "assetName",
            "The selected release has an unsafe source asset name.",
        ));
    }
    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> AppResult<bool> {
    let expected = expected.strip_prefix("sha256:").ok_or_else(|| {
        InstallerError::new("digest", "The release does not provide a SHA-256 digest.")
    })?;
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(InstallerError::new(
            "digest",
            "The release SHA-256 digest is invalid.",
        ));
    }
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()).eq_ignore_ascii_case(expected))
}

fn extract_archive_safely(archive_path: &Path, destination: &Path) -> AppResult<()> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        InstallerError::with_detail(
            "zip",
            "The source archive is not a valid ZIP file.",
            error.to_string(),
        )
    })?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            InstallerError::with_detail(
                "zip",
                "A source archive entry could not be read.",
                error.to_string(),
            )
        })?;
        let enclosed = entry.enclosed_name().ok_or_else(|| {
            InstallerError::new(
                "zipSlip",
                "The source archive contains a path outside its destination.",
            )
        })?;
        if entry
            .unix_mode()
            .map(|mode| mode & 0o170000 == 0o120000)
            .unwrap_or(false)
        {
            return Err(InstallerError::new(
                "zipSymlink",
                "The source archive contains an unsupported symbolic link.",
            ));
        }
        let output = destination.join(enclosed);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output_file = std::fs::File::create(&output)?;
        std::io::copy(&mut entry, &mut output_file)?;
    }
    Ok(())
}

fn find_source_root(work_dir: &Path) -> AppResult<PathBuf> {
    for entry in WalkDir::new(work_dir)
        .max_depth(3)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() && entry.file_name() == "build.sh" {
            let Some(parent) = entry.path().parent() else {
                continue;
            };
            if parent.join("EULA.txt").is_file() && parent.join("CMakeLists.txt").is_file() {
                return Ok(parent.to_path_buf());
            }
        }
    }
    Err(InstallerError::new(
        "sourceLayout",
        "The source archive does not contain the expected Aseprite build files.",
    ))
}

#[cfg(unix)]
fn make_build_script_executable(path: &Path) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

async fn run_build(
    source_root: &Path,
    cancelled: &AtomicBool,
    progress: &Channel<OperationProgress>,
    log_file: &mut std::fs::File,
) -> AppResult<()> {
    let mut command = Command::new("/bin/bash");
    command
        .arg("./build.sh")
        .arg("--auto")
        .arg("--norun")
        .current_dir(source_root)
        .env(
            "PATH",
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        InstallerError::with_detail(
            "buildStart",
            "The official Aseprite build script could not be started.",
            error.to_string(),
        )
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        InstallerError::new("buildOutput", "The build output stream is unavailable.")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        InstallerError::new("buildOutput", "The build error stream is unavailable.")
    })?;
    let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
    let stdout_sender = sender.clone();
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = stdout_sender.send(line);
        }
    });
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = sender.send(line);
        }
    });

    loop {
        if cancelled.load(Ordering::SeqCst) {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(InstallerError::new("cancelled", "The build was cancelled."));
        }
        if let Some(status) = child.try_wait()? {
            while let Ok(line) = receiver.try_recv() {
                writeln!(log_file, "{line}")?;
                let _ = progress.send(OperationProgress::log(OperationStage::Compiling, line));
            }
            if status.success() {
                return Ok(());
            }
            return Err(InstallerError::with_detail(
                "buildFailed",
                "Aseprite’s official build script failed.",
                format!("Exit status: {status}. See {}", log_file_path(log_file)),
            ));
        }
        if let Ok(Some(line)) =
            tokio::time::timeout(std::time::Duration::from_millis(120), receiver.recv()).await
        {
            writeln!(log_file, "{line}")?;
            let _ = progress.send(OperationProgress::log(OperationStage::Compiling, line));
        }
    }
}

fn log_file_path(_: &std::fs::File) -> &'static str {
    "the technical log"
}

async fn install_atomically(
    state: &AppState,
    release: &ReleaseInfo,
    target: &Path,
    built_app: &Path,
    existing: Option<&InstallationInfo>,
    progress: &Channel<OperationProgress>,
) -> AppResult<InstallationInfo> {
    let parent = target.parent().ok_or_else(|| {
        InstallerError::new("target", "The installation path has no parent directory.")
    })?;
    std::fs::create_dir_all(parent)?;
    let suffix = Uuid::new_v4().to_string();
    let staging = parent.join(format!(".aseprite-installer-{suffix}.app"));
    let previous = parent.join(format!(".aseprite-previous-{suffix}.app"));
    let id = installation_id(&target.to_string_lossy());
    let backup = state.paths.backups_dir.join(format!("{id}-previous.app"));

    send_stage(
        progress,
        OperationStage::Installing,
        Some(84),
        "Preparing the new application bundle…",
    );
    copy_bundle(built_app, &staging).await?;
    if !is_aseprite_bundle(&staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(InstallerError::new(
            "staging",
            "The staged application bundle is invalid.",
        ));
    }

    let mut managed = state.load_managed_state()?;
    let current_record = managed
        .installations
        .iter()
        .find(|record| Path::new(&record.path) == target)
        .cloned();
    let mut backup_tag = current_record.as_ref().map(|record| record.tag.clone());
    let mut backup_digest = current_record.as_ref().map(|record| record.digest.clone());
    let mut backup_installed_at = current_record
        .as_ref()
        .map(|record| record.installed_at.clone());
    let mut backup_version_exact = current_record.as_ref().map(|record| record.version_exact);

    if target.exists() {
        send_stage(
            progress,
            OperationStage::BackingUp,
            Some(88),
            "Backing up the current application…",
        );
        if backup.exists() {
            std::fs::remove_dir_all(&backup)?;
        }
        copy_bundle(target, &backup).await?;
        if backup_tag.is_none() {
            backup_tag = existing.and_then(|installation| installation.version.clone());
            backup_installed_at =
                existing.and_then(|installation| installation.installed_at.clone());
            backup_version_exact = existing.map(|installation| installation.version_exact);
            backup_digest = None;
        }
        std::fs::rename(target, &previous)?;
    }

    send_stage(
        progress,
        OperationStage::Installing,
        Some(93),
        "Installing Aseprite in Applications…",
    );
    if let Err(error) = std::fs::rename(&staging, target) {
        if previous.exists() {
            let _ = std::fs::rename(&previous, target);
        }
        return Err(error.into());
    }

    send_stage(
        progress,
        OperationStage::Validating,
        Some(97),
        "Validating the installed application…",
    );
    if !is_aseprite_bundle(target)
        || run_checked(
            "/usr/bin/codesign",
            &[
                "--verify",
                "--deep",
                "--strict",
                target.to_string_lossy().as_ref(),
            ],
            "The installed application failed its signature check.",
        )
        .await
        .is_err()
    {
        let _ = std::fs::remove_dir_all(target);
        if previous.exists() {
            let _ = std::fs::rename(&previous, target);
        }
        return Err(InstallerError::new(
            "installValidation",
            "The new application failed validation; the previous copy was restored.",
        ));
    }
    if previous.exists() {
        std::fs::remove_dir_all(&previous)?;
    }

    let now = Utc::now().to_rfc3339();
    managed
        .installations
        .retain(|record| Path::new(&record.path) != target);
    managed.installations.push(ManagedRecord {
        id: id.clone(),
        path: target.to_string_lossy().into_owned(),
        tag: release.tag.clone(),
        version_exact: true,
        digest: release.digest.clone(),
        architecture: std::env::consts::ARCH.into(),
        installed_at: now.clone(),
        backup_path: backup
            .exists()
            .then(|| backup.to_string_lossy().into_owned()),
        backup_tag,
        backup_digest,
        backup_installed_at,
        backup_version_exact,
    });
    state.save_managed_state(&managed)?;

    send_stage(
        progress,
        OperationStage::Completed,
        Some(100),
        "Aseprite is installed and ready.",
    );
    Ok(InstallationInfo {
        id,
        path: target.to_string_lossy().into_owned(),
        version: Some(release.tag.clone()),
        version_exact: true,
        architecture: Some(std::env::consts::ARCH.into()),
        channel: crate::models::InstallationChannel::Managed,
        manageable: true,
        writable: true,
        has_backup: backup.exists(),
        installed_at: Some(now),
    })
}

pub async fn restore_previous(state: &AppState, id: &str) -> AppResult<InstallationInfo> {
    ensure_aseprite_is_closed()?;
    let mut managed = state.load_managed_state()?;
    let record = managed
        .installations
        .iter_mut()
        .find(|record| record.id == id)
        .ok_or_else(|| InstallerError::new("notManaged", "This installation is not managed."))?;
    let target = PathBuf::from(&record.path);
    let backup = record
        .backup_path
        .as_ref()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .ok_or_else(|| InstallerError::new("noBackup", "No previous backup is available."))?;
    ensure_target_is_safe(&target)?;
    let parent = target
        .parent()
        .ok_or_else(|| InstallerError::new("target", "The target path has no parent."))?;
    let suffix = Uuid::new_v4().to_string();
    let staging = parent.join(format!(".aseprite-restore-{suffix}.app"));
    let current = parent.join(format!(".aseprite-current-{suffix}.app"));
    copy_bundle(&backup, &staging).await?;
    if !is_aseprite_bundle(&staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(InstallerError::new(
            "invalidBackup",
            "The previous backup is not a valid Aseprite application.",
        ));
    }
    std::fs::rename(&target, &current)?;
    if let Err(error) = std::fs::rename(&staging, &target) {
        let _ = std::fs::rename(&current, &target);
        return Err(error.into());
    }
    if backup.exists() {
        std::fs::remove_dir_all(&backup)?;
    }
    copy_bundle(&current, &backup).await?;
    std::fs::remove_dir_all(&current)?;

    std::mem::swap(&mut record.tag, record.backup_tag.get_or_insert_default());
    std::mem::swap(
        &mut record.digest,
        record.backup_digest.get_or_insert_default(),
    );
    std::mem::swap(
        &mut record.installed_at,
        record.backup_installed_at.get_or_insert_default(),
    );
    std::mem::swap(
        &mut record.version_exact,
        record.backup_version_exact.get_or_insert(true),
    );
    record.architecture = std::env::consts::ARCH.into();
    let result = InstallationInfo {
        id: record.id.clone(),
        path: record.path.clone(),
        version: (!record.tag.is_empty()).then(|| record.tag.clone()),
        version_exact: record.version_exact,
        architecture: Some(record.architecture.clone()),
        channel: crate::models::InstallationChannel::Managed,
        manageable: true,
        writable: true,
        has_backup: true,
        installed_at: Some(record.installed_at.clone()),
    };
    state.save_managed_state(&managed)?;
    Ok(result)
}

pub fn uninstall_managed(state: &AppState, id: &str) -> AppResult<()> {
    ensure_aseprite_is_closed()?;
    let mut managed = state.load_managed_state()?;
    let record = managed
        .installations
        .iter()
        .find(|record| record.id == id)
        .cloned()
        .ok_or_else(|| InstallerError::new("notManaged", "This installation is not managed."))?;
    let target = PathBuf::from(&record.path);
    ensure_target_is_safe(&target)?;
    if target.exists() {
        trash::delete(&target).map_err(|error| {
            InstallerError::with_detail(
                "trash",
                "The managed application could not be moved to the Trash.",
                error.to_string(),
            )
        })?;
    }
    if let Some(backup) = record.backup_path.map(PathBuf::from) {
        if backup.starts_with(&state.paths.backups_dir) && backup.exists() {
            std::fs::remove_dir_all(backup)?;
        }
    }
    managed.installations.retain(|entry| entry.id != id);
    state.save_managed_state(&managed)
}

pub fn clean_cache(state: &AppState) -> AppResult<u64> {
    let size = directory_size(&state.paths.cache_dir);
    if state.paths.cache_dir.exists() {
        std::fs::remove_dir_all(&state.paths.cache_dir)?;
    }
    state.paths.ensure()?;
    Ok(size)
}

fn ensure_target_is_safe(target: &Path) -> AppResult<()> {
    if !target.is_absolute()
        || target.file_name().and_then(|name| name.to_str()) != Some("Aseprite.app")
        || target.parent().is_none()
    {
        return Err(InstallerError::new(
            "unsafeTarget",
            "Aseprite can only be installed to an absolute Aseprite.app path.",
        ));
    }
    if target
        .symlink_metadata()
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(InstallerError::new(
            "targetSymlink",
            "A symbolic-link installation target is not supported.",
        ));
    }
    Ok(())
}

fn ensure_aseprite_is_closed() -> AppResult<()> {
    let running = std::process::Command::new("/usr/bin/pgrep")
        .args(["-x", "aseprite"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if running {
        return Err(InstallerError::new(
            "asepriteRunning",
            "Quit Aseprite before replacing, restoring, or removing it.",
        ));
    }
    Ok(())
}

async fn copy_bundle(source: &Path, destination: &Path) -> AppResult<()> {
    if destination.exists() {
        std::fs::remove_dir_all(destination)?;
    }
    run_checked(
        "/usr/bin/ditto",
        &[
            source.to_string_lossy().as_ref(),
            destination.to_string_lossy().as_ref(),
        ],
        "An application bundle could not be copied.",
    )
    .await
}

async fn run_checked(program: &str, arguments: &[&str], message: &str) -> AppResult<()> {
    let output = Command::new(program).args(arguments).output().await?;
    if output.status.success() {
        return Ok(());
    }
    Err(InstallerError::with_detail(
        "commandFailed",
        message,
        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    ))
}

fn ensure_not_cancelled(cancelled: &AtomicBool) -> AppResult<()> {
    if cancelled.load(Ordering::SeqCst) {
        Err(InstallerError::new(
            "cancelled",
            "The operation was cancelled.",
        ))
    } else {
        Ok(())
    }
}

fn send_stage(
    progress: &Channel<OperationProgress>,
    stage: OperationStage,
    percent: Option<u8>,
    message: &str,
) {
    let _ = progress.send(OperationProgress::stage(stage, percent, message));
}

fn prune_files(directory: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, path) in entries.into_iter().skip(keep) {
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn directory_size(directory: &Path) -> u64 {
    WalkDir::new(directory)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_aseprite_bundle(path: &Path) {
        std::fs::create_dir_all(path.join("Contents/Resources")).unwrap();
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert(
            "CFBundleIdentifier".into(),
            Value::String("org.aseprite.Aseprite".into()),
        );
        dictionary.insert(
            "CFBundleIconFile".into(),
            Value::String("aseprite.icns".into()),
        );
        Value::Dictionary(dictionary)
            .to_file_xml(path.join("Contents/Info.plist"))
            .unwrap();
    }

    #[test]
    fn validates_expected_asset_names_only() {
        assert!(validate_asset_name("Aseprite-v1.3.18.1-Source.zip").is_ok());
        assert!(validate_asset_name("../Aseprite-v1.3.18.1-Source.zip").is_err());
        assert!(validate_asset_name("Aseprite-v1.2.40-Source.zip").is_err());
    }

    #[test]
    fn rejects_relative_and_renamed_targets() {
        assert!(ensure_target_is_safe(Path::new("Aseprite.app")).is_err());
        assert!(ensure_target_is_safe(Path::new("/tmp/Other.app")).is_err());
        assert!(ensure_target_is_safe(Path::new("/tmp/Aseprite.app")).is_ok());
    }

    #[test]
    fn verifies_sha256_digest() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("archive.zip");
        std::fs::write(&path, b"aseprite").unwrap();
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(b"aseprite")));
        assert!(verify_sha256(&path, &digest).unwrap());
        assert!(!verify_sha256(&path, &format!("sha256:{}", "0".repeat(64))).unwrap());
    }

    #[test]
    fn applies_the_managed_icon_to_a_built_aseprite_bundle() {
        let directory = tempfile::tempdir().unwrap();
        let bundle = directory.path().join("Aseprite.app");
        write_test_aseprite_bundle(&bundle);

        apply_local_aseprite_icon(&bundle).unwrap();

        assert_eq!(
            std::fs::read(
                bundle
                    .join("Contents/Resources")
                    .join(LOCAL_ASEPRITE_ICON_NAME)
            )
            .unwrap(),
            LOCAL_ASEPRITE_ICON
        );
        let info = Value::from_file(bundle.join("Contents/Info.plist")).unwrap();
        assert_eq!(
            info.as_dictionary()
                .and_then(|dictionary| dictionary.get("CFBundleIconFile"))
                .and_then(Value::as_string),
            Some(LOCAL_ASEPRITE_ICON_NAME)
        );
    }
}
