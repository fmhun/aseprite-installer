use crate::error::{AppResult, InstallerError};
use crate::models::{
    InstallationInfo, ManagedRecord, OperationProgress, OperationStage, ReleaseInfo,
};
use crate::platform::macos::{
    bundle_architecture, bundle_fingerprint, installation_id, is_aseprite_bundle,
    prepare_build_environment, probe_directory_mutation, target_aseprite_running,
    validate_build_environment, BuildEnvironment,
};
use crate::releases::source_build_requirements;
use crate::state::AppState;
use crate::upstream::{ASEPRITE_BUILD_ARGUMENTS, ASEPRITE_BUILD_SCRIPT};
use chrono::Utc;
use fs2::available_space;
use futures_util::StreamExt;
use plist::Value;
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::macos::fs::MetadataExt as MacOsMetadataExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
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
const INSTALL_SPACE_MARGIN_BYTES: u64 = 64 * 1024 * 1024;
const UF_IMMUTABLE: u32 = 0x0000_0002;
const UF_APPEND: u32 = 0x0000_0004;
const SF_IMMUTABLE: u32 = 0x0002_0000;
const SF_APPEND: u32 = 0x0004_0000;
const SF_RESTRICTED: u32 = 0x0008_0000;
const SF_NOUNLINK: u32 = 0x0010_0000;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetSnapshot {
    device: u64,
    inode: u64,
    fingerprint: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectorySnapshot {
    device: u64,
    inode: u64,
    canonical_path: PathBuf,
}

#[derive(Debug, Clone)]
struct InstallSafetyContext {
    target: Option<TargetSnapshot>,
    destination: DirectorySnapshot,
    backup_directory: DirectorySnapshot,
}

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
    let target_snapshot = capture_target_snapshot(target, existing.is_some())?;
    let destination_snapshot = capture_directory_snapshot(
        target
            .parent()
            .ok_or_else(|| InstallerError::new("target", "The target path has no parent."))?,
    )?;
    let backup_directory_snapshot = capture_directory_snapshot(&state.paths.backups_dir)?;
    let safety = InstallSafetyContext {
        target: target_snapshot,
        destination: destination_snapshot,
        backup_directory: backup_directory_snapshot,
    };
    ensure_aseprite_is_closed(target)?;
    let minimum_cmake_version = minimum_cmake_version_for_source_asset(&release.source_asset_name)?;

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
        let cancelled_for_extract = cancelled.clone();
        tauri::async_runtime::spawn_blocking(move || {
            extract_archive_safely(
                &archive_for_extract,
                &work_for_extract,
                &cancelled_for_extract,
            )
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
        ensure_official_build_path_compatible(&source_root)?;
        prepare_isolated_build_configuration(&source_root)?;
        let declared_cmake_version = declared_cmake_minimum(&source_root.join("CMakeLists.txt"))?;
        let effective_cmake_version = minimum_cmake_version.max(declared_cmake_version);
        let build_environment =
            prepare_build_environment(effective_cmake_version, &source_root.join(".build/tools"))?;
        validate_build_environment(&state.paths, &build_environment)?;
        make_build_script_executable(&source_root.join(ASEPRITE_BUILD_SCRIPT))?;
        send_stage(
            progress,
            OperationStage::Compiling,
            None,
            "Compiling Aseprite from official sources…",
        );
        run_build(
            &source_root,
            &build_environment,
            &cancelled,
            progress,
            &mut log_file,
            &log_path,
        )
        .await?;
        ensure_not_cancelled(&cancelled)?;

        let built_app = find_built_aseprite_bundle(&source_root)?;
        ensure_bundle_architecture(&built_app)?;

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

        ensure_not_cancelled(&cancelled)?;
        ensure_aseprite_is_closed(target)?;
        install_atomically(
            state,
            AtomicInstallContext {
                release,
                target,
                built_app: &built_app,
                existing,
                safety: &safety,
                cancelled: &cancelled,
                progress,
            },
        )
        .await
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

fn minimum_cmake_version_for_source_asset(source_asset_name: &str) -> AppResult<[u64; 3]> {
    validate_asset_name(source_asset_name)?;
    source_build_requirements(source_asset_name)
        .map(|requirements| requirements.minimum_cmake_version)
        .ok_or_else(|| {
            InstallerError::new(
                "sourceRequirements",
                "The selected source archive does not have supported build requirements.",
            )
        })
}

fn declared_cmake_minimum(cmake_lists: &Path) -> AppResult<[u64; 3]> {
    let contents = std::fs::read_to_string(cmake_lists).map_err(|error| {
        InstallerError::with_detail(
            "sourceRequirements",
            "The source CMake requirements could not be read.",
            error.to_string(),
        )
    })?;
    for line in contents.lines() {
        let lowercase = line.trim().to_ascii_lowercase();
        if !lowercase.starts_with("cmake_minimum_required") {
            continue;
        }
        let Some((_, after_version)) = lowercase.split_once("version") else {
            continue;
        };
        if let Some(version) = after_version
            .split(|character: char| !character.is_ascii_digit() && character != '.')
            .find(|part| part.contains('.'))
            .and_then(parse_three_part_version)
        {
            return Ok(version);
        }
    }
    Err(InstallerError::new(
        "sourceRequirements",
        "The verified source archive does not declare a readable minimum CMake version.",
    ))
}

fn parse_three_part_version(value: &str) -> Option<[u64; 3]> {
    let mut parts = value.split('.').take(3).map(str::parse::<u64>);
    let major = parts.next()?.ok()?;
    let minor = parts.next()?.ok()?;
    let patch = parts.next().and_then(Result::ok).unwrap_or(0);
    Some([major, minor, patch])
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
    let partial = archive_path.with_extension(format!("zip.{}.part", Uuid::new_v4()));
    let mut partial_cleanup = CleanupFile::new(partial.clone());
    ensure_available_capacity(
        &state.paths.archives_dir,
        release.size.saturating_add(INSTALL_SPACE_MARGIN_BYTES),
        "sourceDownloadSpace",
        "There is not enough free space to download and persist the verified source archive.",
    )?;
    let client = state.http_client()?;
    let mut send = Box::pin(client.get(&release.source_url).send());
    let response = loop {
        ensure_not_cancelled(cancelled)?;
        match tokio::time::timeout(std::time::Duration::from_millis(200), send.as_mut()).await {
            Ok(response) => break response,
            Err(_) => continue,
        }
    }
    .map_err(|error| {
        InstallerError::with_detail(
            "sourceDownload",
            "The official Aseprite source archive could not be downloaded.",
            error.to_string(),
        )
    })?
    .error_for_status()
    .map_err(|error| {
        InstallerError::with_detail(
            "sourceDownload",
            "GitHub did not provide the selected Aseprite source archive.",
            error.to_string(),
        )
    })?;
    let total = response
        .content_length()
        .or(Some(release.size))
        .filter(|size| *size > 0);
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await?;
    partial_cleanup.arm();
    let mut downloaded = 0_u64;
    let mut last_progress = std::time::Instant::now();
    loop {
        ensure_not_cancelled(cancelled)?;
        let next = match tokio::time::timeout(std::time::Duration::from_millis(200), stream.next())
            .await
        {
            Ok(next) => next,
            Err(_) if last_progress.elapsed() < std::time::Duration::from_secs(30) => continue,
            Err(_) => {
                return Err(InstallerError::new(
                    "sourceDownloadTimeout",
                    "The source download stopped responding for 30 seconds. Check the network, proxy, or security software, then retry.",
                ));
            }
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(|error| {
            InstallerError::with_detail(
                "sourceDownload",
                "The official Aseprite source download was interrupted.",
                error.to_string(),
            )
        })?;
        if downloaded.saturating_add(chunk.len() as u64) > release.size {
            return Err(InstallerError::with_detail(
                "sourceSizeMismatch",
                "GitHub sent more source data than the selected release declares.",
                format!("Declared size: {} bytes.", release.size),
            ));
        }
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        last_progress = std::time::Instant::now();
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
    file.sync_all().await?;
    drop(file);
    if downloaded != release.size {
        return Err(InstallerError::with_detail(
            "sourceSizeMismatch",
            "The downloaded source archive size does not match GitHub’s release metadata.",
            format!(
                "Declared: {} bytes; downloaded: {downloaded} bytes.",
                release.size
            ),
        ));
    }

    send_stage(
        progress,
        OperationStage::Verifying,
        Some(27),
        "Verifying the GitHub SHA-256 digest…",
    );
    if !verify_sha256(&partial, &release.digest)? {
        return Err(InstallerError::new(
            "checksumMismatch",
            "The downloaded archive did not match GitHub’s SHA-256 digest.",
        ));
    }
    match rename_exclusive(&partial, &archive_path) {
        Ok(()) => {}
        Err(_) if archive_path.exists() && verify_sha256(&archive_path, &release.digest)? => {}
        Err(error) => {
            return Err(InstallerError::with_detail(
                "sourceCacheCommit",
                "The verified source archive could not be committed to the cache without overwriting another item.",
                format!("{}: {error}", archive_path.display()),
            ));
        }
    }
    std::fs::File::open(&state.paths.archives_dir)?.sync_all()?;
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

fn extract_archive_safely(
    archive_path: &Path,
    destination: &Path,
    cancelled: &AtomicBool,
) -> AppResult<()> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        InstallerError::with_detail(
            "zip",
            "The source archive is not a valid ZIP file.",
            error.to_string(),
        )
    })?;
    for index in 0..archive.len() {
        ensure_not_cancelled(cancelled)?;
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
        let mut buffer = [0_u8; 128 * 1024];
        loop {
            ensure_not_cancelled(cancelled)?;
            let count = entry.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output_file.write_all(&buffer[..count])?;
        }
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
        if entry.file_type().is_file() && entry.file_name() == ASEPRITE_BUILD_SCRIPT {
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

fn prepare_isolated_build_configuration(source_root: &Path) -> AppResult<()> {
    let skia_tag_path = source_root.join("laf/misc/skia-tag.txt");
    let skia_tag = std::fs::read_to_string(&skia_tag_path).map_err(|error| {
        InstallerError::with_detail(
            "skiaTag",
            "The required Skia version could not be read from the source archive.",
            error.to_string(),
        )
    })?;
    let skia_series = skia_tag
        .trim()
        .split('-')
        .next()
        .filter(|series| {
            !series.is_empty() && series.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .ok_or_else(|| {
            InstallerError::new(
                "skiaTag",
                "The source archive declares an invalid Skia version.",
            )
        })?;

    let build_configuration = source_root.join(".build");
    let skia_directory = source_root
        .join(".deps")
        .join(format!("skia-{skia_series}"));
    std::fs::create_dir_all(&build_configuration)?;
    std::fs::create_dir_all(&skia_directory)?;
    std::fs::write(build_configuration.join("userkind"), b"user\n")?;
    write_build_path(&build_configuration.join("builds_dir"), source_root)?;

    // Source release archives have no Git branch, so build.sh uses
    // `_skia_dir`. The named files keep the cache isolated if upstream starts
    // preserving branch metadata in a future source archive.
    for file_name in ["_skia_dir", "main_skia_dir", "beta_skia_dir"] {
        write_build_path(&build_configuration.join(file_name), &skia_directory)?;
    }
    Ok(())
}

fn ensure_official_build_path_compatible(source_root: &Path) -> AppResult<()> {
    if source_root
        .as_os_str()
        .as_bytes()
        .iter()
        .any(u8::is_ascii_whitespace)
    {
        return Err(InstallerError::with_detail(
            "buildPathWhitespace",
            "Aseprite’s official build.sh cannot safely compile from a path containing whitespace.",
            source_root.display().to_string(),
        ));
    }
    Ok(())
}

fn write_build_path(configuration_file: &Path, value: &Path) -> AppResult<()> {
    let value = value.to_str().filter(|value| {
        !value
            .as_bytes()
            .iter()
            .any(|byte| matches!(byte, b'\n' | b'\r'))
    });
    let value = value.ok_or_else(|| {
        InstallerError::new(
            "buildPath",
            "The build workspace path cannot be represented safely.",
        )
    })?;
    std::fs::write(configuration_file, format!("{value}\n"))?;
    Ok(())
}

fn find_built_aseprite_bundle(source_root: &Path) -> AppResult<PathBuf> {
    let bin_directory = source_root.join("build/bin");
    for name in ["aseprite.app", "Aseprite.app"] {
        let candidate = bin_directory.join(name);
        if is_valid_built_bundle(&candidate) {
            return Ok(candidate);
        }
    }

    let build_directory = source_root.join("build");
    let mut candidates = if build_directory.is_dir() {
        WalkDir::new(&build_directory)
            .max_depth(6)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_dir())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case("aseprite.app"))
            })
            .map(|entry| entry.into_path())
            .filter(|path| is_valid_built_bundle(path))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    candidates.sort();
    candidates.dedup();

    match candidates.as_slice() {
        [candidate] => Ok(candidate.clone()),
        [] if case_sensitive_bundle_split(&bin_directory) => Err(InstallerError::new(
            "caseSensitiveBuild",
            "Aseprite’s official build split the executable and resources between aseprite.app and Aseprite.app on this case-sensitive volume.",
        )),
        [] => Err(InstallerError::new(
            "invalidBuild",
            "The build completed without producing a valid Aseprite.app bundle.",
        )),
        _ => Err(InstallerError::new(
            "invalidBuild",
            "The build produced multiple Aseprite.app bundles and none could be selected safely.",
        )),
    }
}

fn is_valid_built_bundle(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false)
        && is_aseprite_bundle(path)
        && [
            "Contents/Resources/data/gui.xml",
            "Contents/Resources/data/pref.xml",
            "Contents/Resources/data/extensions/aseprite-theme/package.json",
        ]
        .iter()
        .all(|relative| path.join(relative).is_file())
}

fn is_restorable_bundle(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false)
        && is_aseprite_bundle(path)
        && bundle_architecture(path).is_some()
}

fn case_sensitive_bundle_split(bin_directory: &Path) -> bool {
    let lowercase = bin_directory.join("aseprite.app");
    let uppercase = bin_directory.join("Aseprite.app");
    lowercase.is_dir()
        && uppercase.is_dir()
        && std::fs::canonicalize(&lowercase).ok() != std::fs::canonicalize(&uppercase).ok()
}

fn ensure_bundle_architecture(app_bundle: &Path) -> AppResult<String> {
    let architectures = bundle_architecture(app_bundle).ok_or_else(|| {
        InstallerError::new(
            "buildArchitecture",
            "The architecture of the built Aseprite executable could not be inspected.",
        )
    })?;
    let expected = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        architecture => architecture,
    };
    let matches_expected = architectures.split_whitespace().any(|architecture| {
        architecture == expected || (expected == "arm64" && architecture == "arm64e")
    });
    if !matches_expected {
        return Err(InstallerError::with_detail(
            "buildArchitecture",
            "The built Aseprite executable does not match this installer architecture.",
            format!("Expected {expected}; built bundle reports {architectures}."),
        ));
    }
    Ok(architectures)
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
    environment: &BuildEnvironment,
    cancelled: &AtomicBool,
    progress: &Channel<OperationProgress>,
    log_file: &mut std::fs::File,
    log_path: &Path,
) -> AppResult<()> {
    let mut command = Command::new("/bin/bash");
    command
        .arg(format!("./{ASEPRITE_BUILD_SCRIPT}"))
        .args(ASEPRITE_BUILD_ARGUMENTS)
        .current_dir(source_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    environment.configure(&mut command);
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
    let process_id = child.id();
    let (sender, mut receiver) = mpsc::channel::<String>(512);
    let stdout_sender = sender.clone();
    let stdout_reader = tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if stdout_sender.send(line).await.is_err() {
                break;
            }
        }
    });
    let stderr_reader = tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if sender.send(line).await.is_err() {
                break;
            }
        }
    });

    loop {
        if cancelled.load(Ordering::SeqCst) {
            terminate_process_group(&mut child).await;
            return Err(InstallerError::new("cancelled", "The build was cancelled."));
        }
        if let Some(status) = child.try_wait()? {
            if let Some(process_id) = process_id {
                signal_process_group(process_id, "-KILL").await;
            }
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                let _ = stdout_reader.await;
                let _ = stderr_reader.await;
            })
            .await;
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
                format!(
                    "Exit status: {status}. Technical log: {}",
                    log_path.display()
                ),
            ));
        }
        if let Ok(Some(line)) =
            tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv()).await
        {
            writeln!(log_file, "{line}")?;
            let _ = progress.send(OperationProgress::log(OperationStage::Compiling, line));
            while let Ok(line) = receiver.try_recv() {
                writeln!(log_file, "{line}")?;
                let _ = progress.send(OperationProgress::log(OperationStage::Compiling, line));
            }
        }
    }
}

async fn terminate_process_group(child: &mut tokio::process::Child) {
    let Some(process_id) = child.id() else {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return;
    };
    signal_process_group(process_id, "-TERM").await;
    for _ in 0..20 {
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    signal_process_group(process_id, "-KILL").await;
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn signal_process_group(process_id: u32, signal: &str) {
    let group = format!("-{process_id}");
    let _ = Command::new("/bin/kill")
        .args([signal, "--", &group])
        .status()
        .await;
}

struct AtomicInstallContext<'a> {
    release: &'a ReleaseInfo,
    target: &'a Path,
    built_app: &'a Path,
    existing: Option<&'a InstallationInfo>,
    safety: &'a InstallSafetyContext,
    cancelled: &'a AtomicBool,
    progress: &'a Channel<OperationProgress>,
}

async fn install_atomically(
    state: &AppState,
    context: AtomicInstallContext<'_>,
) -> AppResult<InstallationInfo> {
    let AtomicInstallContext {
        release,
        target,
        built_app,
        existing,
        safety,
        cancelled,
        progress,
    } = context;
    let target_snapshot = safety.target.as_ref();
    let destination_snapshot = &safety.destination;
    let backup_directory_snapshot = &safety.backup_directory;
    let parent = target.parent().ok_or_else(|| {
        InstallerError::new("target", "The installation path has no parent directory.")
    })?;
    ensure_directory_matches_snapshot(parent, destination_snapshot, "destinationChanged")?;
    ensure_directory_matches_snapshot(
        &state.paths.backups_dir,
        backup_directory_snapshot,
        "backupDirectoryChanged",
    )?;
    let suffix = Uuid::new_v4().to_string();
    let staging = parent.join(format!(".aseprite-installer-{suffix}.app"));
    let previous = parent.join(format!(".aseprite-previous-{suffix}.app"));
    let id = installation_id(&target.to_string_lossy());
    let backup = state.paths.backups_dir.join(format!("{id}-previous.app"));
    let backup_staging = state
        .paths
        .backups_dir
        .join(format!(".{id}-staging-{suffix}.app"));
    let backup_previous = state
        .paths
        .backups_dir
        .join(format!(".{id}-previous-{suffix}.app"));
    let mut staging_cleanup = CleanupDirectory::new(staging.clone());
    let mut backup_staging_cleanup = CleanupDirectory::new(backup_staging.clone());
    let mut previous_cleanup = PreservedDirectory::new(previous.clone());
    let mut backup_previous_cleanup = PreservedDirectory::new(backup_previous.clone());

    ensure_target_matches_snapshot(target, target_snapshot)?;
    ensure_directory_matches_snapshot(parent, destination_snapshot, "destinationChanged")?;
    probe_directory_mutation(parent).map_err(|detail| {
        InstallerError::with_detail(
            "destinationPermissions",
            "The installation destination is no longer writable.",
            detail,
        )
    })?;
    probe_directory_mutation(&state.paths.backups_dir).map_err(|detail| {
        InstallerError::with_detail(
            "backupPermissions",
            "The installer backup folder is no longer writable.",
            detail,
        )
    })?;
    ensure_install_capacity(parent, &state.paths.backups_dir, built_app, target)?;

    let source_version = source_build_requirements(&release.source_asset_name)
        .ok_or_else(|| {
            InstallerError::new(
                "sourceRequirements",
                "The installed source version could not be determined safely.",
            )
        })?
        .source_version
        .to_owned();
    let _registry_lock = state.lock_registry()?;
    let mut managed = state.load_managed_state()?;
    let registered_record = managed
        .installations
        .iter()
        .find(|record| Path::new(&record.path) == target)
        .cloned();
    let current_record = registered_record
        .as_ref()
        .filter(|record| {
            target_snapshot.is_some_and(|snapshot| {
                record
                    .bundle_fingerprint
                    .as_deref()
                    .is_some_and(|fingerprint| {
                        hex::encode(snapshot.fingerprint).eq_ignore_ascii_case(fingerprint)
                    })
            })
        })
        .cloned();
    let existing_backup_snapshot = if backup.exists() {
        let record = registered_record.as_ref().ok_or_else(|| {
            InstallerError::with_detail(
                "unclaimedBackup",
                "An unclaimed rollback bundle occupies the installer’s backup path and was not overwritten.",
                backup.display().to_string(),
            )
        })?;
        let recorded_path = validated_backup_path(state, record)?;
        let snapshot = capture_target_snapshot(&recorded_path, true)?.ok_or_else(|| {
            InstallerError::new(
                "backupIdentityChanged",
                "The recorded rollback backup disappeared.",
            )
        })?;
        ensure_record_fingerprint(
            record.backup_bundle_fingerprint.as_deref(),
            &snapshot.fingerprint,
            "backupIdentityChanged",
            "The existing rollback backup changed outside this installer and was not overwritten.",
        )?;
        Some(snapshot)
    } else {
        None
    };
    let carried_backup = if target_snapshot.is_none() && existing_backup_snapshot.is_some() {
        registered_record.as_ref()
    } else {
        None
    };
    let mut backup_tag = if let Some(record) = carried_backup {
        record.backup_tag.clone()
    } else {
        current_record.as_ref().map(|record| record.tag.clone())
    };
    let mut backup_source_version = if let Some(record) = carried_backup {
        record.backup_source_version.clone()
    } else {
        current_record
            .as_ref()
            .and_then(|record| record.source_version.clone())
    };
    let mut backup_digest = if let Some(record) = carried_backup {
        record.backup_digest.clone()
    } else {
        current_record.as_ref().map(|record| record.digest.clone())
    };
    let mut backup_installed_at = if let Some(record) = carried_backup {
        record.backup_installed_at.clone()
    } else {
        current_record
            .as_ref()
            .map(|record| record.installed_at.clone())
    };
    let mut backup_version_exact = if let Some(record) = carried_backup {
        record.backup_version_exact
    } else {
        current_record.as_ref().map(|record| record.version_exact)
    };
    let mut backup_bundle_fingerprint =
        carried_backup.and_then(|record| record.backup_bundle_fingerprint.clone());
    let mut backup_architecture = if let Some(record) = carried_backup {
        record.backup_architecture.clone()
    } else {
        current_record
            .as_ref()
            .map(|record| record.architecture.clone())
    };

    send_stage(
        progress,
        OperationStage::Installing,
        Some(84),
        "Preparing the new application bundle…",
    );
    copy_bundle(built_app, &staging, &mut staging_cleanup).await?;
    if !is_valid_built_bundle(&staging)
        || bundle_fingerprint(&staging)? != bundle_fingerprint(built_app)?
    {
        return Err(InstallerError::new(
            "staging",
            "The staged application bundle is incomplete or differs from the validated build.",
        ));
    }
    ensure_target_matches_snapshot(target, target_snapshot)?;

    if target_snapshot.is_some() {
        ensure_aseprite_is_closed(target)?;
        ensure_target_matches_snapshot(target, target_snapshot)?;
        ensure_directory_matches_snapshot(parent, destination_snapshot, "destinationChanged")?;
        ensure_directory_matches_snapshot(
            &state.paths.backups_dir,
            backup_directory_snapshot,
            "backupDirectoryChanged",
        )?;
        send_stage(
            progress,
            OperationStage::BackingUp,
            Some(88),
            "Backing up the current application…",
        );
        copy_bundle(target, &backup_staging, &mut backup_staging_cleanup).await?;
        if let Some(snapshot) = target_snapshot {
            if bundle_fingerprint(&backup_staging)? != snapshot.fingerprint {
                return Err(InstallerError::new(
                    "backupValidation",
                    "The previous Aseprite bundle contents could not be backed up safely. The active installation was not changed.",
                ));
            }
        }
        if !is_restorable_bundle(&backup_staging) {
            return Err(InstallerError::new(
                "backupNotRestorable",
                "The existing app copy does not contain an inspectable Aseprite executable, so it cannot be offered as a safe rollback backup.",
            ));
        }
        let copied_backup_architecture = bundle_architecture(&backup_staging).ok_or_else(|| {
            InstallerError::new(
                "backupArchitecture",
                "The previous Aseprite executable architecture could not be inspected.",
            )
        })?;
        ensure_valid_or_ad_hoc_signature(&backup_staging).await?;
        backup_bundle_fingerprint = Some(hex::encode(bundle_fingerprint(&backup_staging)?));
        backup_architecture = Some(copied_backup_architecture);
        if backup_source_version.is_none() {
            backup_source_version = existing.and_then(|installation| installation.version.clone());
        }
        if backup_tag.is_none() {
            backup_tag = existing.and_then(|installation| installation.version.clone());
            backup_installed_at =
                existing.and_then(|installation| installation.installed_at.clone());
            backup_version_exact = existing.map(|installation| installation.version_exact);
            backup_digest = None;
        }
        ensure_aseprite_is_closed(target)?;
        ensure_not_cancelled(cancelled)?;
        send_stage(
            progress,
            OperationStage::Finalizing,
            Some(91),
            "Finalizing the atomic app exchange; cancellation is no longer safe…",
        );
        ensure_target_matches_snapshot(target, target_snapshot)?;
        ensure_directory_matches_snapshot(parent, destination_snapshot, "destinationChanged")?;
        ensure_directory_matches_snapshot(
            &state.paths.backups_dir,
            backup_directory_snapshot,
            "backupDirectoryChanged",
        )?;
        rename_swap(&staging, target).map_err(|error| {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                InstallerError::with_detail(
                    "destinationLocked",
                    "The existing Aseprite app is locked or macOS denied replacing it.",
                    format!("{}: {error}", target.display()),
                )
            } else {
                InstallerError::with_detail(
                    "destinationSwap",
                    "The staged and existing Aseprite apps could not be exchanged atomically.",
                    format!("{}: {error}", target.display()),
                )
            }
        })?;
        if let Some(snapshot) = target_snapshot {
            if let Err(identity_error) =
                ensure_bundle_matches_snapshot(&staging, snapshot, "targetChangedDuringSwap")
            {
                let rollback = rename_swap(&staging, target);
                return Err(match rollback {
                    Ok(()) => identity_error,
                    Err(rollback_error) => InstallerError::with_detail(
                        "rollbackFailed",
                        "The installation target changed at the atomic-swap boundary and could not be exchanged back automatically.",
                        format!(
                            "Both items remain at {} and {}. Identity error: {}. Rollback error: {rollback_error}",
                            target.display(),
                            staging.display(),
                            identity_error
                        ),
                    ),
                });
            }
        }
        if let Err(error) = rename_exclusive(&staging, &previous) {
            let rollback = rename_swap(&staging, target);
            return Err(match rollback {
                Ok(()) => InstallerError::with_detail(
                    "previousPreservation",
                    "The previous app could not be moved to its recoverable transaction path; the atomic exchange was reversed.",
                    error.to_string(),
                ),
                Err(rollback_error) => InstallerError::with_detail(
                    "rollbackFailed",
                    "The previous app could not be preserved after the atomic exchange, and the exchange could not be reversed automatically.",
                    format!(
                        "Old copy: {}. New copy: {}. Preservation error: {error}. Rollback error: {rollback_error}",
                        staging.display(),
                        target.display()
                    ),
                ),
            });
        }
        previous_cleanup.arm();
        if let Some(snapshot) = target_snapshot {
            ensure_bundle_matches_snapshot(&previous, snapshot, "targetChangedAfterMove")?;
        }
    }

    if target_snapshot.is_none() {
        ensure_not_cancelled(cancelled)?;
    }
    send_stage(
        progress,
        OperationStage::Finalizing,
        Some(93),
        "Committing Aseprite and its rollback state…",
    );
    ensure_directory_matches_snapshot(parent, destination_snapshot, "destinationChanged")?;
    if target_snapshot.is_none() {
        ensure_target_presence_is_safe(target, false)?;
        if let Err(error) = rename_exclusive(&staging, target) {
            return Err(InstallerError::with_detail(
                "destinationCommit",
                "The staged Aseprite app could not be committed to the destination.",
                format!("{}: {error}", target.display()),
            ));
        }
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
        return Err(rollback_or_error(
            target,
            &previous,
            &staging,
            InstallerError::new(
                "installValidation",
                "The new application failed validation; the previous copy was restored.",
            ),
        ));
    }
    let installed_fingerprint = match bundle_fingerprint(target) {
        Ok(fingerprint) => hex::encode(fingerprint),
        Err(error) => {
            return Err(rollback_or_error(target, &previous, &staging, error));
        }
    };
    let installed_architecture = match bundle_architecture(target) {
        Some(architecture) => architecture,
        None => {
            return Err(rollback_or_error(
                target,
                &previous,
                &staging,
                InstallerError::new(
                    "installArchitecture",
                    "The installed Aseprite architecture could not be verified.",
                ),
            ));
        }
    };

    let backup_committed = if target_snapshot.is_some() {
        if let Err(error) = ensure_directory_matches_snapshot(
            &state.paths.backups_dir,
            backup_directory_snapshot,
            "backupDirectoryChanged",
        ) {
            return Err(rollback_or_error(target, &previous, &staging, error));
        }
        if backup.exists() {
            if let Some(snapshot) = existing_backup_snapshot.as_ref() {
                if let Err(error) =
                    ensure_bundle_matches_snapshot(&backup, snapshot, "backupIdentityChanged")
                {
                    return Err(rollback_or_error(target, &previous, &staging, error));
                }
            }
            if backup
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(rollback_or_error(
                    target,
                    &previous,
                    &staging,
                    InstallerError::new(
                        "backupSymlink",
                        "The existing rollback backup is a symbolic link and was not replaced.",
                    ),
                ));
            }
            if let Err(error) = ensure_tree_is_mutable(&backup) {
                return Err(rollback_or_error(target, &previous, &staging, error));
            }
            if let Err(error) = rename_swap(&backup_staging, &backup) {
                return Err(rollback_or_error(
                    target,
                    &previous,
                    &staging,
                    InstallerError::with_detail(
                        "backupLocked",
                        "The existing and new rollback backups could not be exchanged atomically.",
                        format!("{}: {error}", backup.display()),
                    ),
                ));
            }
            if let Some(snapshot) = existing_backup_snapshot.as_ref() {
                if let Err(identity_error) = ensure_bundle_matches_snapshot(
                    &backup_staging,
                    snapshot,
                    "backupIdentityChangedDuringSwap",
                ) {
                    let restore_error = rename_swap(&backup_staging, &backup).err();
                    let original = InstallerError::with_detail(
                        "backupIdentityChangedDuringSwap",
                        "The rollback backup changed at the atomic-swap boundary and was not discarded.",
                        match restore_error {
                            Some(error) => format!(
                                "Both items remain at {} and {}. Identity error: {}. Restore error: {error}",
                                backup.display(),
                                backup_staging.display(),
                                identity_error
                            ),
                            None => identity_error.to_string(),
                        },
                    );
                    return Err(rollback_or_error(target, &previous, &staging, original));
                }
            }
            if let Err(error) = rename_exclusive(&backup_staging, &backup_previous) {
                let rollback_error = rename_swap(&backup_staging, &backup).err();
                return Err(rollback_or_error(
                    target,
                    &previous,
                    &staging,
                    InstallerError::with_detail(
                        "backupPreservation",
                        "The old rollback backup could not be moved to its recoverable transaction path.",
                        match rollback_error {
                            Some(rollback_error) => format!(
                                "Preservation error: {error}. The atomic backup exchange could not be reversed: {rollback_error}."
                            ),
                            None => format!(
                                "Preservation error: {error}. The atomic backup exchange was reversed."
                            ),
                        },
                    ),
                ));
            }
            backup_previous_cleanup.arm();
            if let Some(snapshot) = existing_backup_snapshot.as_ref() {
                if let Err(identity_error) = ensure_bundle_matches_snapshot(
                    &backup_previous,
                    snapshot,
                    "backupIdentityChangedAfterMove",
                ) {
                    let restore_error = rename_exclusive(&backup_previous, &backup_staging)
                        .and_then(|_| rename_swap(&backup_staging, &backup))
                        .err();
                    return Err(rollback_or_error(
                        target,
                        &previous,
                        &staging,
                        InstallerError::with_detail(
                            "backupIdentityChangedAfterMove",
                            "The old rollback backup changed while it was being preserved and was not discarded.",
                            match restore_error {
                                Some(error) => format!(
                                    "The recoverable items remain at {} and/or {}. Identity error: {}. Restore error: {error}",
                                    backup_previous.display(),
                                    backup_staging.display(),
                                    identity_error
                                ),
                                None => identity_error.to_string(),
                            },
                        ),
                    ));
                }
            }
        } else if let Err(error) = rename_exclusive(&backup_staging, &backup) {
            return Err(rollback_or_error(
                target,
                &previous,
                &staging,
                InstallerError::with_detail(
                    "backupCommit",
                    "The new rollback backup could not be committed safely.",
                    error.to_string(),
                ),
            ));
        }
        true
    } else {
        false
    };

    let now = Utc::now().to_rfc3339();
    managed
        .installations
        .retain(|record| Path::new(&record.path) != target);
    managed.schema_version = 2;
    managed.installations.push(ManagedRecord {
        id: id.clone(),
        path: target.to_string_lossy().into_owned(),
        tag: release.tag.clone(),
        source_version: Some(source_version.clone()),
        version_exact: true,
        digest: release.digest.clone(),
        architecture: installed_architecture.clone(),
        installed_at: now.clone(),
        bundle_fingerprint: Some(installed_fingerprint),
        backup_path: (backup.exists() && backup_bundle_fingerprint.is_some())
            .then(|| backup.to_string_lossy().into_owned()),
        backup_tag,
        backup_source_version,
        backup_digest,
        backup_installed_at,
        backup_version_exact,
        backup_bundle_fingerprint,
        backup_architecture,
        integration_paths: Vec::new(),
    });
    if let Err(error) = state.save_managed_state(&managed) {
        let backup_rollback =
            rollback_backup_swap(&backup, &backup_previous, &backup_staging, backup_committed);
        let target_rollback = rollback_target_swap(target, &previous, &staging);
        if backup_rollback.is_err() || target_rollback.is_err() {
            return Err(InstallerError::with_detail(
                "rollbackFailed",
                "The registry could not be saved and the previous installation could not be restored completely.",
                format!(
                    "Registry error: {error}. Target rollback: {}. Backup rollback: {}.",
                    target_rollback
                        .err()
                        .unwrap_or_else(|| "succeeded".into()),
                    backup_rollback
                        .err()
                        .unwrap_or_else(|| "succeeded".into())
                ),
            ));
        }
        return Err(error);
    }
    previous_cleanup.remove_after_commit();
    backup_previous_cleanup.remove_after_commit();

    send_stage(
        progress,
        OperationStage::Completed,
        Some(100),
        "Aseprite is installed and ready.",
    );
    Ok(InstallationInfo {
        id,
        path: target.to_string_lossy().into_owned(),
        version: Some(source_version),
        version_exact: true,
        architecture: Some(installed_architecture),
        channel: crate::models::InstallationChannel::Managed,
        manageable: true,
        writable: true,
        has_backup: backup.exists(),
        installed_at: Some(now),
    })
}

fn rollback_target_swap(target: &Path, previous: &Path, failed_new: &Path) -> Result<(), String> {
    if target.symlink_metadata().is_ok() && previous.symlink_metadata().is_ok() {
        if failed_new.symlink_metadata().is_ok() {
            return Err(format!(
                "Cannot preserve the failed new app because {} is occupied.",
                failed_new.display()
            ));
        }
        rename_swap(target, previous).map_err(|error| {
            format!(
                "Could not atomically restore {} over {}: {error}",
                previous.display(),
                target.display()
            )
        })?;
        return rename_exclusive(previous, failed_new).map_err(|error| {
            format!(
                "The previous app was restored, but the failed new app could not be preserved from {} to {}: {error}",
                previous.display(),
                failed_new.display()
            )
        });
    }
    if target.symlink_metadata().is_ok() {
        if failed_new.symlink_metadata().is_ok() {
            return Err(format!(
                "Cannot preserve the failed new app because {} is occupied.",
                failed_new.display()
            ));
        }
        rename_exclusive(target, failed_new).map_err(|error| {
            format!(
                "Could not move the failed new app from {} to {}: {error}",
                target.display(),
                failed_new.display()
            )
        })?;
    }
    if previous.symlink_metadata().is_ok() {
        rename_exclusive(previous, target).map_err(|error| {
            format!(
                "Could not restore {} to {}: {error}",
                previous.display(),
                target.display()
            )
        })?;
    }
    Ok(())
}

fn rollback_or_error(
    target: &Path,
    previous: &Path,
    failed_new: &Path,
    original: InstallerError,
) -> InstallerError {
    match rollback_target_swap(target, previous, failed_new) {
        Ok(()) => original,
        Err(rollback) => InstallerError::with_detail(
            "rollbackFailed",
            "The installation failed and the previous Aseprite app could not be restored automatically.",
            format!(
                "Original error: {}{}. Rollback error: {rollback}",
                original.message,
                original
                    .detail
                    .as_deref()
                    .map(|detail| format!(" ({detail})"))
                    .unwrap_or_default()
            ),
        ),
    }
}

fn rollback_backup_swap(
    backup: &Path,
    backup_previous: &Path,
    failed_new_backup: &Path,
    committed: bool,
) -> Result<(), String> {
    if committed && backup.symlink_metadata().is_ok() && backup_previous.symlink_metadata().is_ok()
    {
        if failed_new_backup.symlink_metadata().is_ok() {
            return Err(format!(
                "Cannot preserve the new backup because {} is occupied.",
                failed_new_backup.display()
            ));
        }
        rename_swap(backup, backup_previous).map_err(|error| {
            format!(
                "Could not atomically restore the previous backup from {} to {}: {error}",
                backup_previous.display(),
                backup.display()
            )
        })?;
        return rename_exclusive(backup_previous, failed_new_backup).map_err(|error| {
            format!(
                "The previous backup was restored, but the new backup could not be preserved from {} to {}: {error}",
                backup_previous.display(),
                failed_new_backup.display()
            )
        });
    }
    if committed && backup.symlink_metadata().is_ok() {
        if failed_new_backup.symlink_metadata().is_ok() {
            return Err(format!(
                "Cannot preserve the new backup because {} is occupied.",
                failed_new_backup.display()
            ));
        }
        rename_exclusive(backup, failed_new_backup).map_err(|error| {
            format!(
                "Could not move the new backup from {} to {}: {error}",
                backup.display(),
                failed_new_backup.display()
            )
        })?;
    }
    if backup_previous.symlink_metadata().is_ok() {
        rename_exclusive(backup_previous, backup).map_err(|error| {
            format!(
                "Could not restore the previous backup from {} to {}: {error}",
                backup_previous.display(),
                backup.display()
            )
        })?;
    }
    Ok(())
}

fn rename_exclusive(source: &Path, destination: &Path) -> std::io::Result<()> {
    rename_with_flags(source, destination, 0x0000_0004)
}

fn rename_swap(source: &Path, destination: &Path) -> std::io::Result<()> {
    rename_with_flags(source, destination, 0x0000_0002)
}

fn rename_with_flags(source: &Path, destination: &Path, flags: u32) -> std::io::Result<()> {
    const AT_FDCWD: i32 = -2;
    unsafe extern "C" {
        fn renameatx_np(
            from_fd: i32,
            from: *const std::os::raw::c_char,
            to_fd: i32,
            to: *const std::os::raw::c_char,
            flags: u32,
        ) -> i32;
    }

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: both C strings are NUL-terminated and live for the duration of
    // the call. The caller supplies either RENAME_EXCL (collision-safe move)
    // or RENAME_SWAP (atomic exchange with no missing-target window).
    let result = unsafe {
        renameatx_np(
            AT_FDCWD,
            source.as_ptr(),
            AT_FDCWD,
            destination.as_ptr(),
            flags,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub async fn restore_previous(state: &AppState, id: &str) -> AppResult<InstallationInfo> {
    let _registry_lock = state.lock_registry()?;
    let mut managed = state.load_managed_state()?;
    let record_index = managed
        .installations
        .iter()
        .position(|record| record.id == id)
        .ok_or_else(|| InstallerError::new("notManaged", "This installation is not managed."))?;
    let record = managed.installations[record_index].clone();
    validate_managed_record_id(&record)?;
    let target = PathBuf::from(&record.path);
    ensure_target_is_safe(&target)?;
    let backup = validated_backup_path(state, &record)?;
    if !backup.exists() {
        return Err(InstallerError::new(
            "noBackup",
            "No previous backup is available.",
        ));
    }
    let target_snapshot = capture_target_snapshot(&target, true)?.ok_or_else(|| {
        InstallerError::new("targetChanged", "The managed Aseprite app is missing.")
    })?;
    ensure_record_fingerprint(
        record.bundle_fingerprint.as_deref(),
        &target_snapshot.fingerprint,
        "managedIdentityChanged",
        "The managed Aseprite app was replaced or changed outside this installer. It was not restored over.",
    )?;
    let backup_snapshot = capture_target_snapshot(&backup, true)?
        .ok_or_else(|| InstallerError::new("noBackup", "The previous backup is unavailable."))?;
    ensure_record_fingerprint(
        record.backup_bundle_fingerprint.as_deref(),
        &backup_snapshot.fingerprint,
        "backupIdentityChanged",
        "The rollback backup changed outside this installer and cannot be restored safely.",
    )?;
    ensure_aseprite_is_closed(&target)?;

    let parent = target
        .parent()
        .ok_or_else(|| InstallerError::new("target", "The target path has no parent."))?;
    let destination_snapshot = capture_directory_snapshot(parent)?;
    let backup_directory_snapshot = capture_directory_snapshot(&state.paths.backups_dir)?;
    probe_directory_mutation(parent).map_err(|detail| {
        InstallerError::with_detail(
            "destinationPermissions",
            "The installation destination is not writable.",
            detail,
        )
    })?;
    probe_directory_mutation(&state.paths.backups_dir).map_err(|detail| {
        InstallerError::with_detail(
            "backupPermissions",
            "The installer backup folder is not writable.",
            detail,
        )
    })?;
    ensure_restore_capacity(parent, &state.paths.backups_dir, &target, &backup)?;

    let suffix = Uuid::new_v4().to_string();
    let staging = parent.join(format!(".aseprite-restore-{suffix}.app"));
    let current = parent.join(format!(".aseprite-current-{suffix}.app"));
    let backup_staging = state
        .paths
        .backups_dir
        .join(format!(".{id}-restore-staging-{suffix}.app"));
    let backup_previous = state
        .paths
        .backups_dir
        .join(format!(".{id}-restore-previous-{suffix}.app"));
    let mut staging_cleanup = CleanupDirectory::new(staging.clone());
    let mut backup_staging_cleanup = CleanupDirectory::new(backup_staging.clone());
    let mut current_cleanup = PreservedDirectory::new(current.clone());
    let mut backup_previous_cleanup = PreservedDirectory::new(backup_previous.clone());

    copy_bundle(&backup, &staging, &mut staging_cleanup).await?;
    copy_bundle(&target, &backup_staging, &mut backup_staging_cleanup).await?;
    if !is_restorable_bundle(&staging)
        || bundle_fingerprint(&staging)? != backup_snapshot.fingerprint
    {
        return Err(InstallerError::new(
            "invalidBackup",
            "The staged rollback backup is incomplete or no longer matches the verified backup.",
        ));
    }
    if bundle_fingerprint(&backup_staging)? != target_snapshot.fingerprint {
        return Err(InstallerError::new(
            "backupValidation",
            "The current Aseprite app could not be staged as the next rollback backup.",
        ));
    }
    let restored_architecture = bundle_architecture(&staging).ok_or_else(|| {
        InstallerError::new(
            "backupArchitecture",
            "The rollback backup architecture could not be inspected.",
        )
    })?;
    ensure_valid_or_ad_hoc_signature(&staging).await?;
    let restored_fingerprint = bundle_fingerprint(&staging)?;

    ensure_aseprite_is_closed(&target)?;
    ensure_directory_matches_snapshot(parent, &destination_snapshot, "destinationChanged")?;
    ensure_directory_matches_snapshot(
        &state.paths.backups_dir,
        &backup_directory_snapshot,
        "backupDirectoryChanged",
    )?;
    ensure_target_matches_snapshot(&target, Some(&target_snapshot))?;
    ensure_bundle_matches_snapshot(&backup, &backup_snapshot, "backupIdentityChanged")?;

    rename_swap(&staging, &target).map_err(|error| {
        InstallerError::with_detail(
            "restoreTargetSwap",
            "The current and rollback Aseprite apps could not be exchanged atomically.",
            error.to_string(),
        )
    })?;
    if let Err(identity_error) =
        ensure_bundle_matches_snapshot(&staging, &target_snapshot, "targetChangedDuringSwap")
    {
        let rollback = rename_swap(&staging, &target);
        return Err(match rollback {
            Ok(()) => identity_error,
            Err(rollback_error) => InstallerError::with_detail(
                "rollbackFailed",
                "The restore target changed at the atomic-swap boundary and could not be exchanged back automatically.",
                format!(
                    "Both items remain at {} and {}. Identity error: {}. Rollback error: {rollback_error}",
                    target.display(),
                    staging.display(),
                    identity_error
                ),
            ),
        });
    }
    if let Err(error) = rename_exclusive(&staging, &current) {
        let rollback = rename_swap(&staging, &target);
        return Err(match rollback {
            Ok(()) => InstallerError::with_detail(
                "restoreCurrentPreservation",
                "The current app could not be moved to its recoverable transaction path; the atomic restore was reversed.",
                error.to_string(),
            ),
            Err(rollback_error) => InstallerError::with_detail(
                "rollbackFailed",
                "The current app could not be preserved after the atomic restore, and the exchange could not be reversed automatically.",
                format!(
                    "Current copy: {}. Restored copy: {}. Preservation error: {error}. Rollback error: {rollback_error}",
                    staging.display(),
                    target.display()
                ),
            ),
        });
    }
    current_cleanup.arm();
    if let Err(identity_error) =
        ensure_bundle_matches_snapshot(&current, &target_snapshot, "targetChangedAfterMove")
    {
        let rollback = rollback_target_swap(&target, &current, &staging);
        return Err(match rollback {
            Ok(()) => identity_error,
            Err(rollback_error) => InstallerError::with_detail(
                "rollbackFailed",
                "The current app changed while it was being preserved and the atomic restore could not be reversed completely.",
                format!(
                    "Identity error: {}. Rollback error: {rollback_error}",
                    identity_error
                ),
            ),
        });
    }

    if let Err(error) = rename_swap(&backup_staging, &backup) {
        return Err(rollback_or_error(
            &target,
            &current,
            &staging,
            InstallerError::with_detail(
                "restoreBackupSwap",
                "The existing and next rollback backups could not be exchanged atomically.",
                error.to_string(),
            ),
        ));
    }
    if let Err(identity_error) = ensure_bundle_matches_snapshot(
        &backup_staging,
        &backup_snapshot,
        "backupIdentityChangedDuringSwap",
    ) {
        let backup_rollback =
            rename_swap(&backup_staging, &backup).map_err(|error| error.to_string());
        let target_rollback = rollback_target_swap(&target, &current, &staging);
        return Err(combine_restore_rollback_error(
            InstallerError::with_detail(
                "backupIdentityChangedDuringSwap",
                "The rollback backup changed at the atomic-swap boundary and was not discarded.",
                identity_error.to_string(),
            ),
            backup_rollback,
            target_rollback,
        ));
    }
    if let Err(error) = rename_exclusive(&backup_staging, &backup_previous) {
        let backup_rollback =
            rename_swap(&backup_staging, &backup).map_err(|error| error.to_string());
        let target_rollback = rollback_target_swap(&target, &current, &staging);
        return Err(combine_restore_rollback_error(
            InstallerError::with_detail(
                "restoreBackupPreservation",
                "The previous rollback backup could not be moved to its recoverable transaction path.",
                error.to_string(),
            ),
            backup_rollback,
            target_rollback,
        ));
    }
    backup_previous_cleanup.arm();
    if let Err(identity_error) = ensure_bundle_matches_snapshot(
        &backup_previous,
        &backup_snapshot,
        "backupIdentityChangedAfterMove",
    ) {
        let backup_rollback = rename_exclusive(&backup_previous, &backup_staging)
            .and_then(|_| rename_swap(&backup_staging, &backup))
            .map_err(|error| error.to_string());
        let target_rollback = rollback_target_swap(&target, &current, &staging);
        return Err(combine_restore_rollback_error(
            identity_error,
            backup_rollback,
            target_rollback,
        ));
    }

    if let Err(error) = run_checked(
        "/usr/bin/codesign",
        &[
            "--verify",
            "--deep",
            "--strict",
            target.to_string_lossy().as_ref(),
        ],
        "The restored Aseprite app failed its signature check.",
    )
    .await
    {
        let backup_rollback =
            rollback_backup_swap(&backup, &backup_previous, &backup_staging, true);
        let target_rollback = rollback_target_swap(&target, &current, &staging);
        return Err(combine_restore_rollback_error(
            error,
            backup_rollback,
            target_rollback,
        ));
    }

    let mut updated = record.clone();
    let restored_source_version = updated
        .backup_source_version
        .clone()
        .or_else(|| bundle_short_version(&target));
    let restored_tag = updated
        .backup_tag
        .clone()
        .unwrap_or_else(|| restored_source_version.clone().unwrap_or_default());
    let restored_digest = updated.backup_digest.clone().unwrap_or_default();
    let restored_installed_at = updated
        .backup_installed_at
        .clone()
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let restored_version_exact = updated
        .backup_version_exact
        .unwrap_or(restored_source_version.is_some());

    updated.backup_tag = Some(updated.tag.clone());
    updated.backup_source_version = updated.source_version.clone();
    updated.backup_digest = Some(updated.digest.clone());
    updated.backup_installed_at = Some(updated.installed_at.clone());
    updated.backup_version_exact = Some(updated.version_exact);
    updated.backup_bundle_fingerprint = updated.bundle_fingerprint.clone();
    updated.backup_architecture = Some(updated.architecture.clone());
    updated.tag = restored_tag;
    updated.source_version = restored_source_version;
    updated.digest = restored_digest;
    updated.installed_at = restored_installed_at;
    updated.version_exact = restored_version_exact;
    updated.bundle_fingerprint = Some(hex::encode(restored_fingerprint));
    updated.architecture = restored_architecture.clone();
    managed.installations[record_index] = updated.clone();
    managed.schema_version = 2;

    if let Err(error) = state.save_managed_state(&managed) {
        let backup_rollback =
            rollback_backup_swap(&backup, &backup_previous, &backup_staging, true);
        let target_rollback = rollback_target_swap(&target, &current, &staging);
        return Err(combine_restore_rollback_error(
            error,
            backup_rollback,
            target_rollback,
        ));
    }
    current_cleanup.remove_after_commit();
    backup_previous_cleanup.remove_after_commit();
    let result = InstallationInfo {
        id: updated.id.clone(),
        path: updated.path.clone(),
        version: updated
            .source_version
            .clone()
            .or_else(|| (!updated.tag.is_empty()).then(|| updated.tag.clone())),
        version_exact: updated.version_exact,
        architecture: Some(restored_architecture),
        channel: crate::models::InstallationChannel::Managed,
        manageable: true,
        writable: true,
        has_backup: true,
        installed_at: Some(updated.installed_at.clone()),
    };
    Ok(result)
}

fn validate_managed_record_id(record: &ManagedRecord) -> AppResult<()> {
    let suffix = record.id.strip_prefix("aseprite-");
    if suffix.is_none_or(|value| {
        value.len() != 16 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) || record.id != installation_id(&record.path)
    {
        return Err(InstallerError::new(
            "managedRecordInvalid",
            "The managed installation record has an invalid identity and was not used.",
        ));
    }
    Ok(())
}

fn validated_backup_path(state: &AppState, record: &ManagedRecord) -> AppResult<PathBuf> {
    let expected = state
        .paths
        .backups_dir
        .join(format!("{}-previous.app", record.id));
    let recorded = record
        .backup_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| InstallerError::new("noBackup", "No previous backup is available."))?;
    if recorded != expected {
        return Err(InstallerError::with_detail(
            "backupPathInvalid",
            "The rollback backup path is outside the installer’s managed location.",
            format!(
                "Expected {}; registry contains {}.",
                expected.display(),
                recorded.display()
            ),
        ));
    }
    if recorded
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(InstallerError::new(
            "backupSymlink",
            "A symbolic-link rollback backup is not supported.",
        ));
    }
    Ok(recorded)
}

fn ensure_record_fingerprint(
    expected: Option<&str>,
    actual: &[u8; 32],
    code: &str,
    message: &str,
) -> AppResult<()> {
    let expected = expected.ok_or_else(|| {
        InstallerError::new(
            "managedIdentityMissing",
            "This installation predates identity tracking. Re-adopt it before a destructive managed action.",
        )
    })?;
    if !hex::encode(actual).eq_ignore_ascii_case(expected) {
        return Err(InstallerError::new(code, message));
    }
    Ok(())
}

fn ensure_bundle_matches_snapshot(
    bundle: &Path,
    expected: &TargetSnapshot,
    code: &str,
) -> AppResult<()> {
    let actual = capture_target_snapshot(bundle, true)?;
    if actual.as_ref() != Some(expected) {
        return Err(InstallerError::new(
            code,
            "An Aseprite bundle changed while the operation was running. Nothing was overwritten in its place.",
        ));
    }
    Ok(())
}

fn ensure_restore_capacity(
    destination_directory: &Path,
    backup_directory: &Path,
    target: &Path,
    backup: &Path,
) -> AppResult<()> {
    let current_bytes = checked_directory_size(target)?;
    let backup_bytes = checked_directory_size(backup)?;
    let same_volume = std::fs::metadata(destination_directory)?.dev()
        == std::fs::metadata(backup_directory)?.dev();
    if same_volume {
        ensure_available_capacity(
            destination_directory,
            current_bytes
                .saturating_add(backup_bytes)
                .saturating_add(INSTALL_SPACE_MARGIN_BYTES),
            "restoreSpace",
            "There is not enough free space to stage both sides of the rollback transaction.",
        )
    } else {
        ensure_available_capacity(
            destination_directory,
            backup_bytes.saturating_add(INSTALL_SPACE_MARGIN_BYTES),
            "restoreDestinationSpace",
            "There is not enough destination space to stage the previous Aseprite app.",
        )?;
        ensure_available_capacity(
            backup_directory,
            current_bytes.saturating_add(INSTALL_SPACE_MARGIN_BYTES),
            "restoreBackupSpace",
            "There is not enough backup space to preserve the current Aseprite app.",
        )
    }
}

fn bundle_short_version(bundle: &Path) -> Option<String> {
    Value::from_file(bundle.join("Contents/Info.plist"))
        .ok()?
        .as_dictionary()?
        .get("CFBundleShortVersionString")?
        .as_string()
        .map(str::to_owned)
}

fn combine_restore_rollback_error(
    original: InstallerError,
    backup_rollback: Result<(), String>,
    target_rollback: Result<(), String>,
) -> InstallerError {
    if backup_rollback.is_ok() && target_rollback.is_ok() {
        return original;
    }
    InstallerError::with_detail(
        "rollbackFailed",
        "The restore failed and the original installation state could not be recovered completely.",
        format!(
            "Original error: {}{}. Backup rollback: {}. Target rollback: {}.",
            original.message,
            original
                .detail
                .as_deref()
                .map(|detail| format!(" ({detail})"))
                .unwrap_or_default(),
            backup_rollback.err().unwrap_or_else(|| "succeeded".into()),
            target_rollback.err().unwrap_or_else(|| "succeeded".into())
        ),
    )
}

pub fn uninstall_managed(state: &AppState, id: &str) -> AppResult<()> {
    let _registry_lock = state.lock_registry()?;
    let mut managed = state.load_managed_state()?;
    let record = managed
        .installations
        .iter()
        .find(|record| record.id == id)
        .cloned()
        .ok_or_else(|| InstallerError::new("notManaged", "This installation is not managed."))?;
    validate_managed_record_id(&record)?;
    let target = PathBuf::from(&record.path);
    ensure_target_is_safe(&target)?;
    let target_snapshot = capture_target_snapshot(&target, true)?.ok_or_else(|| {
        InstallerError::new("targetChanged", "The managed Aseprite app is missing.")
    })?;
    ensure_record_fingerprint(
        record.bundle_fingerprint.as_deref(),
        &target_snapshot.fingerprint,
        "managedIdentityChanged",
        "The managed Aseprite app was replaced or changed outside this installer. It was not removed.",
    )?;
    ensure_aseprite_is_closed(&target)?;

    let parent = target
        .parent()
        .ok_or_else(|| InstallerError::new("target", "The target path has no parent."))?;
    probe_directory_mutation(parent).map_err(|detail| {
        InstallerError::with_detail(
            "destinationPermissions",
            "The installation destination is not writable.",
            detail,
        )
    })?;
    let destination_snapshot = capture_directory_snapshot(parent)?;

    let backup = if record.backup_path.is_some() {
        let path = validated_backup_path(state, &record)?;
        if path.exists() {
            let snapshot = capture_target_snapshot(&path, true)?.ok_or_else(|| {
                InstallerError::new("backupIdentityChanged", "The rollback backup disappeared.")
            })?;
            ensure_record_fingerprint(
                record.backup_bundle_fingerprint.as_deref(),
                &snapshot.fingerprint,
                "backupIdentityChanged",
                "The rollback backup changed outside this installer and was not removed.",
            )?;
            Some((path, snapshot))
        } else {
            None
        }
    } else {
        None
    };
    if backup.is_some() {
        probe_directory_mutation(&state.paths.backups_dir).map_err(|detail| {
            InstallerError::with_detail(
                "backupPermissions",
                "The installer backup folder is not writable.",
                detail,
            )
        })?;
    }
    let backup_directory_snapshot = backup
        .as_ref()
        .map(|_| capture_directory_snapshot(&state.paths.backups_dir))
        .transpose()?;

    ensure_aseprite_is_closed(&target)?;
    ensure_directory_matches_snapshot(parent, &destination_snapshot, "destinationChanged")?;
    ensure_target_matches_snapshot(&target, Some(&target_snapshot))?;
    if let (Some((path, snapshot)), Some(directory_snapshot)) =
        (backup.as_ref(), backup_directory_snapshot.as_ref())
    {
        ensure_directory_matches_snapshot(
            &state.paths.backups_dir,
            directory_snapshot,
            "backupDirectoryChanged",
        )?;
        ensure_bundle_matches_snapshot(path, snapshot, "backupIdentityChanged")?;
    }

    managed.installations.retain(|entry| entry.id != id);
    managed.schema_version = 2;
    state.save_managed_state(&managed)?;

    // Registry-first removal is deliberately crash-safe: until the Trash
    // operation succeeds the app remains visible as a manual installation,
    // rather than disappearing into an internal hidden transaction path.
    ensure_aseprite_is_closed(&target).map_err(|error| {
        InstallerError::with_detail(
            "uninstallDeferred",
            "The app was unmanaged, but it became busy before it could be moved to the Trash. It remains installed as a manual copy.",
            error.to_string(),
        )
    })?;
    ensure_directory_matches_snapshot(parent, &destination_snapshot, "destinationChanged")?;
    ensure_bundle_matches_snapshot(&target, &target_snapshot, "targetChanged")?;
    if let Err(error) = trash::delete(&target) {
        return Err(InstallerError::with_detail(
            "uninstallCleanup",
            "The app was unmanaged but could not be moved to the Trash. It remains at its original path as a manual installation.",
            format!("{}: {error}", target.display()),
        ));
    }

    if let (Some((backup, snapshot)), Some(directory_snapshot)) =
        (backup.as_ref(), backup_directory_snapshot.as_ref())
    {
        ensure_directory_matches_snapshot(
            &state.paths.backups_dir,
            directory_snapshot,
            "backupDirectoryChanged",
        )?;
        ensure_bundle_matches_snapshot(backup, snapshot, "backupIdentityChanged")?;
        if let Err(error) = trash::delete(backup) {
            return Err(InstallerError::with_detail(
                "uninstallBackupCleanup",
                "Aseprite was moved to the Trash, but its verified rollback backup could not be moved there too.",
                format!("{}: {error}", backup.display()),
            ));
        }
    }
    Ok(())
}

pub fn clean_cache(state: &AppState) -> AppResult<u64> {
    let size = directory_size(&state.paths.cache_dir);
    if state.paths.cache_dir.exists() {
        std::fs::remove_dir_all(&state.paths.cache_dir)?;
    }
    state.paths.ensure()?;
    Ok(size)
}

fn ensure_target_presence_is_safe(target: &Path, expected_to_exist: bool) -> AppResult<()> {
    match std::fs::symlink_metadata(target) {
        Ok(_) if !expected_to_exist => Err(InstallerError::new(
            "targetOccupied",
            "An item appeared at the installation destination. It was not overwritten.",
        )),
        Ok(metadata) if !metadata.file_type().is_dir() || !is_aseprite_bundle(target) => {
            Err(InstallerError::new(
                "targetChanged",
                "The existing installation changed or is no longer a valid Aseprite app. It was not overwritten.",
            ))
        }
        Ok(_) => ensure_tree_is_mutable(target),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !expected_to_exist => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(InstallerError::new(
            "targetChanged",
            "The existing Aseprite installation disappeared during the operation. Nothing was installed in its place.",
        )),
        Err(error) => Err(InstallerError::with_detail(
            "targetInspect",
            "The installation destination could not be inspected safely.",
            error.to_string(),
        )),
    }
}

fn ensure_tree_is_mutable(root: &Path) -> AppResult<()> {
    const PROTECTED_FLAGS: u32 =
        UF_IMMUTABLE | UF_APPEND | SF_IMMUTABLE | SF_APPEND | SF_RESTRICTED | SF_NOUNLINK;
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| {
            InstallerError::with_detail(
                "destinationLocked",
                "The Aseprite app could not be inspected for locked files.",
                error.to_string(),
            )
        })?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        let flags = MacOsMetadataExt::st_flags(&metadata);
        if flags & PROTECTED_FLAGS == 0 {
            continue;
        }
        let system_protected =
            flags & (SF_IMMUTABLE | SF_APPEND | SF_RESTRICTED | SF_NOUNLINK) != 0;
        return Err(InstallerError::with_detail(
            "destinationLocked",
            "The Aseprite app contains a locked item and cannot be replaced safely.",
            if system_protected {
                format!(
                    "{} has a system immutable/append/no-unlink/restricted flag. An administrator or device-management policy must remove it; Finder may not be sufficient.",
                    entry.path().display()
                )
            } else {
                format!(
                    "{} has the Locked/append flag. Remove it in Finder’s Get Info window (or with chflags), then check again.",
                    entry.path().display()
                )
            },
        ));
    }
    Ok(())
}

fn capture_target_snapshot(
    target: &Path,
    expected_to_exist: bool,
) -> AppResult<Option<TargetSnapshot>> {
    ensure_target_presence_is_safe(target, expected_to_exist)?;
    if !expected_to_exist {
        return Ok(None);
    }
    let metadata = std::fs::symlink_metadata(target)?;
    Ok(Some(TargetSnapshot {
        device: metadata.dev(),
        inode: metadata.ino(),
        fingerprint: bundle_fingerprint(target)?,
    }))
}

fn ensure_target_matches_snapshot(
    target: &Path,
    expected: Option<&TargetSnapshot>,
) -> AppResult<()> {
    let current = capture_target_snapshot(target, expected.is_some())?;
    if current.as_ref() != expected {
        return Err(InstallerError::new(
            "targetChanged",
            "The existing Aseprite installation changed during compilation. The changed copy was not overwritten.",
        ));
    }
    Ok(())
}

fn capture_directory_snapshot(directory: &Path) -> AppResult<DirectorySnapshot> {
    let canonical_path = std::fs::canonicalize(directory).map_err(|error| {
        InstallerError::with_detail(
            "directoryInspect",
            "An installer working directory could not be resolved safely.",
            format!("{}: {error}", directory.display()),
        )
    })?;
    let metadata = std::fs::metadata(directory).map_err(|error| {
        InstallerError::with_detail(
            "directoryInspect",
            "An installer working directory could not be inspected safely.",
            format!("{}: {error}", directory.display()),
        )
    })?;
    if !metadata.is_dir() {
        return Err(InstallerError::new(
            "directoryInspect",
            "An installer working path is no longer a directory.",
        ));
    }
    Ok(DirectorySnapshot {
        device: metadata.dev(),
        inode: metadata.ino(),
        canonical_path,
    })
}

fn ensure_directory_matches_snapshot(
    directory: &Path,
    expected: &DirectorySnapshot,
    code: &str,
) -> AppResult<()> {
    let current = capture_directory_snapshot(directory)?;
    if &current != expected {
        return Err(InstallerError::with_detail(
            code,
            "An installer destination changed while the operation was running.",
            format!(
                "Expected {}; now resolves to {}. Nothing was overwritten.",
                expected.canonical_path.display(),
                current.canonical_path.display()
            ),
        ));
    }
    Ok(())
}

fn ensure_install_capacity(
    destination_directory: &Path,
    backup_directory: &Path,
    built_app: &Path,
    target: &Path,
) -> AppResult<()> {
    let built_bytes = checked_directory_size(built_app)?;
    let current_bytes = if target.exists() {
        checked_directory_size(target)?
    } else {
        0
    };
    let same_volume = std::fs::metadata(destination_directory)?.dev()
        == std::fs::metadata(backup_directory)?.dev();

    if same_volume {
        let required = built_bytes
            .saturating_add(current_bytes)
            .saturating_add(INSTALL_SPACE_MARGIN_BYTES);
        ensure_available_capacity(
            destination_directory,
            required,
            "destinationSpace",
            "There is not enough free space to stage Aseprite and preserve the previous copy.",
        )?;
    } else {
        ensure_available_capacity(
            destination_directory,
            built_bytes.saturating_add(INSTALL_SPACE_MARGIN_BYTES),
            "destinationSpace",
            "There is not enough free space to stage Aseprite on the destination volume.",
        )?;
        if current_bytes > 0 {
            ensure_available_capacity(
                backup_directory,
                current_bytes.saturating_add(INSTALL_SPACE_MARGIN_BYTES),
                "backupSpace",
                "There is not enough free space to preserve the previous Aseprite copy on the backup volume.",
            )?;
        }
    }
    Ok(())
}

fn ensure_available_capacity(
    directory: &Path,
    required: u64,
    code: &str,
    message: &str,
) -> AppResult<()> {
    let available = available_space(directory).map_err(|error| {
        InstallerError::with_detail(
            code,
            "Free space on an installation volume could not be inspected.",
            format!("{}: {error}", directory.display()),
        )
    })?;
    if available < required {
        return Err(InstallerError::with_detail(
            code,
            message,
            format!(
                "{} has {:.1} MB available; {:.1} MB is required at this step.",
                directory.display(),
                available as f64 / 1024_f64.powi(2),
                required as f64 / 1024_f64.powi(2)
            ),
        ));
    }
    Ok(())
}

fn checked_directory_size(directory: &Path) -> AppResult<u64> {
    let mut size = 0_u64;
    for entry in WalkDir::new(directory).follow_links(false) {
        let entry = entry.map_err(|error| {
            InstallerError::with_detail(
                "sizeInspection",
                "An application bundle could not be measured before installation.",
                error.to_string(),
            )
        })?;
        let metadata = entry.metadata().map_err(|error| {
            InstallerError::with_detail(
                "sizeInspection",
                "An application bundle could not be measured before installation.",
                error.to_string(),
            )
        })?;
        if metadata.is_file() {
            size = size.saturating_add(metadata.len());
        }
    }
    Ok(size)
}

fn ensure_target_is_safe(target: &Path) -> AppResult<()> {
    if !target.is_absolute()
        || !target
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        || target.parent().is_none()
    {
        return Err(InstallerError::new(
            "unsafeTarget",
            "Aseprite can only be installed to an absolute application-bundle path.",
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

fn ensure_aseprite_is_closed(target: &Path) -> AppResult<()> {
    if target_aseprite_running(target).map_err(|detail| {
        InstallerError::with_detail(
            "asepriteProcessInspect",
            "Running Aseprite processes could not be inspected safely.",
            detail,
        )
    })? {
        return Err(InstallerError::new(
            "asepriteRunning",
            "Quit the selected Aseprite copy before replacing, restoring, or removing it.",
        ));
    }
    Ok(())
}

async fn copy_bundle(
    source: &Path,
    destination: &Path,
    cleanup: &mut CleanupDirectory,
) -> AppResult<()> {
    match std::fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(InstallerError::with_detail(
                "stagingOccupied",
                "A temporary bundle destination is unexpectedly occupied and was not overwritten.",
                destination.display().to_string(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    std::fs::create_dir(destination)?;
    cleanup.arm()?;
    let mut command = Command::new("/usr/bin/ditto");
    command
        .args([
            "--rsrc",
            "--extattr",
            "--qtn",
            "--acl",
            source.to_string_lossy().as_ref(),
            destination.to_string_lossy().as_ref(),
        ])
        .env_remove("DITTONORSRC")
        .env_remove("DITTOABORT")
        .env_remove("DITTOKEEPBINARIESPATTERN")
        .env_remove("DITTOKEEPBINARIESDIR")
        .env_remove("DITTO_TEST_OPTIONS");
    let output = command_output_with_timeout(
        command,
        std::time::Duration::from_secs(30 * 60),
        "bundleCopyTimeout",
        "Copying an application bundle did not finish within 30 minutes.",
    )
    .await?;
    if output.status.success() {
        let source_permissions = std::fs::symlink_metadata(source)?.permissions();
        std::fs::set_permissions(destination, source_permissions)?;
        Ok(())
    } else {
        Err(InstallerError::with_detail(
            "bundleCopy",
            "An application bundle could not be copied.",
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

async fn run_checked(program: &str, arguments: &[&str], message: &str) -> AppResult<()> {
    let mut command = Command::new(program);
    command.args(arguments);
    let output = command_output_with_timeout(
        command,
        std::time::Duration::from_secs(5 * 60),
        "commandTimeout",
        "A required macOS validation command did not finish within five minutes.",
    )
    .await?;
    if output.status.success() {
        return Ok(());
    }
    Err(InstallerError::with_detail(
        "commandFailed",
        message,
        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    ))
}

async fn command_output_with_timeout(
    mut command: Command,
    timeout: std::time::Duration,
    code: &str,
    timeout_message: &str,
) -> AppResult<std::process::Output> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let child = command.spawn()?;
    let process_id = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(output) => Ok(output?),
        Err(_) => {
            if let Some(process_id) = process_id {
                signal_process_group(process_id, "-KILL").await;
            }
            Err(InstallerError::new(code, timeout_message))
        }
    }
}

async fn ensure_valid_or_ad_hoc_signature(bundle: &Path) -> AppResult<()> {
    let bundle = bundle.to_string_lossy().into_owned();
    if run_checked(
        "/usr/bin/codesign",
        &["--verify", "--deep", "--strict", &bundle],
        "The rollback copy did not have a valid code signature.",
    )
    .await
    .is_ok()
    {
        return Ok(());
    }
    run_checked(
        "/usr/bin/codesign",
        &["--force", "--deep", "--sign", "-", &bundle],
        "The rollback copy was unsigned and could not be signed locally.",
    )
    .await?;
    run_checked(
        "/usr/bin/codesign",
        &["--verify", "--deep", "--strict", &bundle],
        "The locally signed rollback copy failed validation.",
    )
    .await
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

struct CleanupDirectory {
    path: PathBuf,
    identity: Option<(u64, u64)>,
}

struct CleanupFile {
    path: PathBuf,
    identity: Option<(u64, u64)>,
}

impl CleanupFile {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            identity: None,
        }
    }

    fn arm(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_file() {
                self.identity = Some((metadata.dev(), metadata.ino()));
            }
        }
    }
}

impl Drop for CleanupFile {
    fn drop(&mut self) {
        let Some(expected) = self.identity else {
            return;
        };
        let current = std::fs::symlink_metadata(&self.path)
            .ok()
            .filter(|metadata| metadata.file_type().is_file())
            .map(|metadata| (metadata.dev(), metadata.ino()));
        if current == Some(expected) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl CleanupDirectory {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            identity: None,
        }
    }

    fn arm(&mut self) -> AppResult<()> {
        let metadata = std::fs::symlink_metadata(&self.path)?;
        if !metadata.file_type().is_dir() {
            return Err(InstallerError::with_detail(
                "transactionCleanup",
                "A newly created transaction path is not a directory.",
                self.path.display().to_string(),
            ));
        }
        self.identity = Some((metadata.dev(), metadata.ino()));
        Ok(())
    }
}

impl Drop for CleanupDirectory {
    fn drop(&mut self) {
        let Some(expected) = self.identity else {
            return;
        };
        let current = std::fs::symlink_metadata(&self.path)
            .ok()
            .map(|metadata| (metadata.dev(), metadata.ino()));
        if current == Some(expected) {
            if let Err(error) = std::fs::remove_dir_all(&self.path) {
                eprintln!(
                    "Aseprite Installer left a recoverable transaction directory at {}: {error}",
                    self.path.display()
                );
            }
        }
    }
}

struct PreservedDirectory {
    path: PathBuf,
    identity: Option<(u64, u64)>,
}

impl PreservedDirectory {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            identity: None,
        }
    }

    fn arm(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_dir() {
                self.identity = Some((metadata.dev(), metadata.ino()));
            }
        }
    }

    fn remove_after_commit(&mut self) {
        let Some(expected) = self.identity.take() else {
            return;
        };
        let current = std::fs::symlink_metadata(&self.path)
            .ok()
            .filter(|metadata| metadata.file_type().is_dir())
            .map(|metadata| (metadata.dev(), metadata.ino()));
        if current != Some(expected) {
            return;
        }
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            eprintln!(
                "Aseprite Installer left a recoverable committed transaction copy at {}: {error}",
                self.path.display()
            );
        }
    }
}

impl Drop for PreservedDirectory {
    fn drop(&mut self) {
        let Some(expected) = self.identity else {
            return;
        };
        let current = std::fs::symlink_metadata(&self.path)
            .ok()
            .filter(|metadata| metadata.file_type().is_dir())
            .map(|metadata| (metadata.dev(), metadata.ino()));
        if current == Some(expected) {
            eprintln!(
                "Aseprite Installer preserved a recoverable transaction copy at {}.",
                self.path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_marker_directory(path: &Path, marker: &[u8]) {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("marker"), marker).unwrap();
    }

    fn read_marker_directory(path: &Path) -> Vec<u8> {
        std::fs::read(path.join("marker")).unwrap()
    }

    fn test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn write_test_aseprite_bundle(path: &Path) {
        std::fs::create_dir_all(path.join("Contents/Resources")).unwrap();
        for relative in [
            "Contents/Resources/data/gui.xml",
            "Contents/Resources/data/pref.xml",
            "Contents/Resources/data/extensions/aseprite-theme/package.json",
        ] {
            let resource = path.join(relative);
            std::fs::create_dir_all(resource.parent().unwrap()).unwrap();
            std::fs::write(resource, b"test").unwrap();
        }
        std::fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
        std::fs::copy("/usr/bin/true", path.join("Contents/MacOS/aseprite")).unwrap();
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert(
            "CFBundleIdentifier".into(),
            Value::String("org.aseprite.Aseprite".into()),
        );
        dictionary.insert(
            "CFBundleIconFile".into(),
            Value::String("aseprite.icns".into()),
        );
        dictionary.insert(
            "CFBundleExecutable".into(),
            Value::String("aseprite".into()),
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
    fn uses_cmake_requirement_from_the_actual_source_asset() {
        assert!(minimum_cmake_version_for_source_asset("Aseprite-v1.3.14.4-Source.zip").is_err());
        assert_eq!(
            minimum_cmake_version_for_source_asset("Aseprite-v1.3.15.5-Source.zip").unwrap(),
            [3, 20, 0]
        );
        assert!(
            minimum_cmake_version_for_source_asset("Aseprite-v1.3.invalid-Source.zip").is_err()
        );
    }

    #[test]
    fn revalidates_the_cmake_minimum_declared_by_extracted_sources() {
        let directory = tempfile::tempdir().unwrap();
        let cmake_lists = directory.path().join("CMakeLists.txt");
        std::fs::write(
            &cmake_lists,
            "# verified source\ncmake_minimum_required(VERSION 3.27.4)\nproject(aseprite)\n",
        )
        .unwrap();
        assert_eq!(declared_cmake_minimum(&cmake_lists).unwrap(), [3, 27, 4]);

        std::fs::write(
            &cmake_lists,
            "cmake_minimum_required(VERSION 3.20...3.31)\n",
        )
        .unwrap();
        assert_eq!(declared_cmake_minimum(&cmake_lists).unwrap(), [3, 20, 0]);
    }

    #[test]
    fn accepts_safe_absolute_app_names_and_rejects_other_targets() {
        assert!(ensure_target_is_safe(Path::new("Aseprite.app")).is_err());
        assert!(ensure_target_is_safe(Path::new("/tmp/not-an-app")).is_err());
        assert!(ensure_target_is_safe(Path::new("/tmp/Other.app")).is_ok());
        assert!(ensure_target_is_safe(Path::new("/tmp/Aseprite.app")).is_ok());
    }

    #[test]
    fn cleanup_file_only_removes_the_file_it_created() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("download.part");
        std::fs::write(&path, b"partial").unwrap();
        let mut cleanup = CleanupFile::new(path.clone());
        cleanup.arm();
        drop(cleanup);
        assert!(!path.exists());

        std::fs::write(&path, b"first").unwrap();
        let mut cleanup = CleanupFile::new(path.clone());
        cleanup.arm();
        let replacement = directory.path().join("replacement.part");
        std::fs::write(&replacement, b"replacement").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(replacement, &path).unwrap();
        drop(cleanup);
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    }

    #[test]
    fn rollback_copies_are_preserved_until_a_commit_explicitly_cleans_them() {
        let directory = tempfile::tempdir().unwrap();
        let preserved = directory.path().join("previous.app");
        std::fs::create_dir(&preserved).unwrap();
        std::fs::write(preserved.join("marker"), b"old app").unwrap();
        let mut guard = PreservedDirectory::new(preserved.clone());
        guard.arm();
        drop(guard);
        assert_eq!(std::fs::read(preserved.join("marker")).unwrap(), b"old app");

        let mut guard = PreservedDirectory::new(preserved.clone());
        guard.arm();
        guard.remove_after_commit();
        assert!(!preserved.exists());
    }

    #[test]
    fn atomically_exchanges_both_directory_names() {
        let directory = tempfile::tempdir().unwrap();
        let left = directory.path().join("left.app");
        let right = directory.path().join("right.app");
        write_marker_directory(&left, b"left bundle");
        write_marker_directory(&right, b"right bundle");

        rename_swap(&left, &right).unwrap();

        assert!(left.is_dir());
        assert!(right.is_dir());
        assert_eq!(read_marker_directory(&left), b"right bundle");
        assert_eq!(read_marker_directory(&right), b"left bundle");
    }

    #[test]
    fn exclusive_rename_refuses_an_occupied_destination() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.app");
        let destination = directory.path().join("destination.app");
        write_marker_directory(&source, b"source bundle");
        write_marker_directory(&destination, b"destination bundle");

        assert!(rename_exclusive(&source, &destination).is_err());

        assert_eq!(read_marker_directory(&source), b"source bundle");
        assert_eq!(read_marker_directory(&destination), b"destination bundle");
    }

    #[test]
    fn target_rollback_restores_the_previous_bundle_and_preserves_the_failed_new_one() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("Aseprite.app");
        let previous = directory.path().join("previous.app");
        let failed_new = directory.path().join("failed-new.app");
        write_marker_directory(&target, b"new bundle");
        write_marker_directory(&previous, b"previous bundle");

        rollback_target_swap(&target, &previous, &failed_new).unwrap();

        assert_eq!(read_marker_directory(&target), b"previous bundle");
        assert!(!previous.exists());
        assert_eq!(read_marker_directory(&failed_new), b"new bundle");
    }

    #[test]
    fn target_rollback_vacates_a_failed_fresh_install_destination() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("Aseprite.app");
        let previous = directory.path().join("previous.app");
        let failed_new = directory.path().join("failed-new.app");
        write_marker_directory(&target, b"new bundle");

        rollback_target_swap(&target, &previous, &failed_new).unwrap();

        assert!(!target.exists());
        assert!(!previous.exists());
        assert_eq!(read_marker_directory(&failed_new), b"new bundle");
    }

    #[test]
    fn target_rollback_refuses_to_overwrite_an_existing_recovery_copy() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("Aseprite.app");
        let previous = directory.path().join("previous.app");
        let failed_new = directory.path().join("failed-new.app");
        write_marker_directory(&target, b"new bundle");
        write_marker_directory(&previous, b"previous bundle");
        write_marker_directory(&failed_new, b"unrelated bundle");

        assert!(rollback_target_swap(&target, &previous, &failed_new).is_err());

        assert_eq!(read_marker_directory(&target), b"new bundle");
        assert_eq!(read_marker_directory(&previous), b"previous bundle");
        assert_eq!(read_marker_directory(&failed_new), b"unrelated bundle");
    }

    #[test]
    fn backup_rollback_restores_the_previous_backup_and_preserves_the_new_one() {
        let directory = tempfile::tempdir().unwrap();
        let backup = directory.path().join("backup.app");
        let backup_previous = directory.path().join("backup-previous.app");
        let failed_new_backup = directory.path().join("failed-new-backup.app");
        write_marker_directory(&backup, b"new backup");
        write_marker_directory(&backup_previous, b"previous backup");

        rollback_backup_swap(&backup, &backup_previous, &failed_new_backup, true).unwrap();

        assert_eq!(read_marker_directory(&backup), b"previous backup");
        assert!(!backup_previous.exists());
        assert_eq!(read_marker_directory(&failed_new_backup), b"new backup");
    }

    #[test]
    fn backup_rollback_vacates_a_newly_created_backup_path() {
        let directory = tempfile::tempdir().unwrap();
        let backup = directory.path().join("backup.app");
        let backup_previous = directory.path().join("backup-previous.app");
        let failed_new_backup = directory.path().join("failed-new-backup.app");
        write_marker_directory(&backup, b"new backup");

        rollback_backup_swap(&backup, &backup_previous, &failed_new_backup, true).unwrap();

        assert!(!backup.exists());
        assert!(!backup_previous.exists());
        assert_eq!(read_marker_directory(&failed_new_backup), b"new backup");
    }

    #[test]
    fn backup_rollback_refuses_to_overwrite_an_existing_recovery_copy() {
        let directory = tempfile::tempdir().unwrap();
        let backup = directory.path().join("backup.app");
        let backup_previous = directory.path().join("backup-previous.app");
        let failed_new_backup = directory.path().join("failed-new-backup.app");
        write_marker_directory(&backup, b"new backup");
        write_marker_directory(&backup_previous, b"previous backup");
        write_marker_directory(&failed_new_backup, b"unrelated bundle");

        assert!(rollback_backup_swap(&backup, &backup_previous, &failed_new_backup, true).is_err());

        assert_eq!(read_marker_directory(&backup), b"new backup");
        assert_eq!(read_marker_directory(&backup_previous), b"previous backup");
        assert_eq!(
            read_marker_directory(&failed_new_backup),
            b"unrelated bundle"
        );
    }

    #[test]
    fn cancellation_before_extraction_creates_no_archive_entries() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("source.zip");
        let archive_file = std::fs::File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(archive_file);
        archive
            .start_file(
                "Aseprite-source/file.txt",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"archive contents").unwrap();
        archive.finish().unwrap();
        let destination = directory.path().join("extract");
        let cancelled = AtomicBool::new(true);

        let error = extract_archive_safely(&archive_path, &destination, &cancelled).unwrap_err();

        assert_eq!(error.code, "cancelled");
        assert!(!destination.join("Aseprite-source/file.txt").exists());
    }

    #[test]
    fn command_timeout_kills_the_spawned_process_group() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("orphan-marker");
        let marker_argument = marker.to_string_lossy().into_owned();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "(/bin/sleep 1; /usr/bin/touch \"$1\") & wait",
            "aseprite-timeout-probe",
            &marker_argument,
        ]);

        let error = test_runtime()
            .block_on(command_output_with_timeout(
                command,
                std::time::Duration::from_millis(100),
                "testTimeout",
                "The test command timed out.",
            ))
            .unwrap_err();

        assert_eq!(error.code, "testTimeout");
        std::thread::sleep(std::time::Duration::from_millis(1_200));
        assert!(!marker.exists());
    }

    #[test]
    fn committed_cleanup_never_removes_a_replacement_transaction_path() {
        let directory = tempfile::tempdir().unwrap();
        let preserved = directory.path().join("previous.app");
        let replacement = directory.path().join("replacement.app");
        std::fs::create_dir(&preserved).unwrap();
        let mut guard = PreservedDirectory::new(preserved.clone());
        guard.arm();
        std::fs::create_dir(&replacement).unwrap();
        std::fs::write(replacement.join("marker"), b"replacement").unwrap();
        std::fs::remove_dir(&preserved).unwrap();
        std::fs::rename(replacement, &preserved).unwrap();
        guard.remove_after_commit();
        assert_eq!(
            std::fs::read(preserved.join("marker")).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn refuses_destination_changes_between_preflight_and_installation() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("Aseprite.app");
        assert!(ensure_target_presence_is_safe(&target, false).is_ok());

        std::fs::write(&target, b"unrelated file").unwrap();
        assert_eq!(
            ensure_target_presence_is_safe(&target, false)
                .unwrap_err()
                .code,
            "targetOccupied"
        );
        std::fs::remove_file(&target).unwrap();

        write_test_aseprite_bundle(&target);
        assert!(ensure_target_presence_is_safe(&target, true).is_ok());
        let snapshot = capture_target_snapshot(&target, true).unwrap().unwrap();
        std::fs::remove_dir_all(&target).unwrap();
        write_test_aseprite_bundle(&target);
        std::fs::write(target.join("Contents/changed"), b"replacement").unwrap();
        assert_eq!(
            ensure_target_matches_snapshot(&target, Some(&snapshot))
                .unwrap_err()
                .code,
            "targetChanged"
        );
    }

    #[test]
    fn measures_bundle_contents_for_the_runtime_space_check() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("first"), vec![0_u8; 17]).unwrap();
        std::fs::create_dir(directory.path().join("nested")).unwrap();
        std::fs::write(directory.path().join("nested/second"), vec![0_u8; 23]).unwrap();
        assert_eq!(checked_directory_size(directory.path()).unwrap(), 40);
    }

    #[test]
    fn accepts_a_built_bundle_containing_the_native_architecture() {
        let directory = tempfile::tempdir().unwrap();
        let bundle = directory.path().join("Aseprite.app");
        write_test_aseprite_bundle(&bundle);
        assert!(!ensure_bundle_architecture(&bundle).unwrap().is_empty());
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

    #[test]
    fn isolates_the_official_build_and_skia_directories() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        std::fs::create_dir_all(source.join("laf/misc")).unwrap();
        std::fs::write(source.join("laf/misc/skia-tag.txt"), b"m124-08a5439a6b\n").unwrap();

        prepare_isolated_build_configuration(&source).unwrap();

        assert_eq!(
            std::fs::read_to_string(source.join(".build/userkind")).unwrap(),
            "user\n"
        );
        assert_eq!(
            std::fs::read_to_string(source.join(".build/builds_dir")).unwrap(),
            format!("{}\n", source.display())
        );
        let expected_skia = source.join(".deps/skia-m124");
        assert!(expected_skia.is_dir());
        for file_name in ["_skia_dir", "main_skia_dir", "beta_skia_dir"] {
            assert_eq!(
                std::fs::read_to_string(source.join(".build").join(file_name)).unwrap(),
                format!("{}\n", expected_skia.display())
            );
        }
    }

    #[test]
    fn rejects_whitespace_that_the_official_build_script_cannot_quote_safely() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source with spaces");
        assert_eq!(
            ensure_official_build_path_compatible(&source)
                .unwrap_err()
                .code,
            "buildPathWhitespace"
        );
    }

    #[test]
    fn prefers_the_lowercase_bundle_emitted_by_the_build() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path();
        let lowercase = source.join("build/bin/aseprite.app");
        let uppercase = source.join("build/bin/Aseprite.app");
        write_test_aseprite_bundle(&lowercase);
        write_test_aseprite_bundle(&uppercase);

        assert_eq!(find_built_aseprite_bundle(source).unwrap(), lowercase);
    }

    #[test]
    fn finds_a_single_valid_bundle_in_a_fallback_build_location() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path();
        let fallback = source.join("build/release/bin/ASEPRITE.app");
        write_test_aseprite_bundle(&fallback);

        assert_eq!(find_built_aseprite_bundle(source).unwrap(), fallback);
    }

    #[test]
    fn rejects_a_bundle_missing_runtime_data_resources() {
        let directory = tempfile::tempdir().unwrap();
        let bundle = directory.path().join("build/bin/aseprite.app");
        write_test_aseprite_bundle(&bundle);
        std::fs::remove_file(bundle.join("Contents/Resources/data/gui.xml")).unwrap();

        assert!(!is_valid_built_bundle(&bundle));
        assert_eq!(
            find_built_aseprite_bundle(directory.path())
                .unwrap_err()
                .code,
            "invalidBuild"
        );
    }

    #[test]
    fn accepts_a_legacy_bundle_for_restore_without_treating_it_as_a_current_build() {
        let directory = tempfile::tempdir().unwrap();
        let bundle = directory.path().join("Aseprite.app");
        write_test_aseprite_bundle(&bundle);
        std::fs::remove_dir_all(bundle.join("Contents/Resources/data")).unwrap();

        assert!(!is_valid_built_bundle(&bundle));
        assert!(is_restorable_bundle(&bundle));
    }

    #[test]
    fn rejects_ambiguous_fallback_build_bundles() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path();
        write_test_aseprite_bundle(&source.join("build/one/aseprite.app"));
        write_test_aseprite_bundle(&source.join("build/two/Aseprite.app"));

        assert!(find_built_aseprite_bundle(source).is_err());
    }
}
