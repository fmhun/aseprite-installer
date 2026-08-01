use crate::error::{AppResult, InstallerError};
use crate::models::{
    InstallationChannel, InstallationInfo, ManagedRecord, OperationProgress, OperationStage,
    ReleaseInfo, MANAGED_STATE_SCHEMA_VERSION,
};
#[cfg(target_os = "linux")]
use crate::platform::linux as native;
#[cfg(target_os = "windows")]
use crate::platform::windows as native;
use crate::portable_transaction::{
    durable_rename_file_no_replace, durable_rename_no_replace, load_journal, managed_state_sha256,
    recovery_direction, remove_journal, sync_tree_durable, write_journal, InstallJournal,
    IntegrationSnapshot, JournalOperation, JournalPhase, PortableJournal, QuarantineEntry,
    RecoveryDirection, RestoreJournal, UninstallJournal, PORTABLE_JOURNAL_SCHEMA_VERSION,
};
use crate::releases::{portable_source_supported, source_build_requirements};
use crate::state::{AppState, CommitDurability};
use chrono::Utc;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsStr;
#[cfg(target_os = "linux")]
use std::ffi::{CStr, CString, OsString};
use std::fs::OpenOptions;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as UnixMetadataExt, OpenOptionsExt};
#[cfg(target_os = "linux")]
use std::os::unix::{
    ffi::{OsStrExt, OsStringExt},
    io::{AsRawFd, FromRawFd, OwnedFd, RawFd},
};
#[cfg(target_os = "windows")]
use std::os::windows::{
    ffi::{OsStrExt as WindowsOsStrExt, OsStringExt as WindowsOsStringExt},
    fs::{MetadataExt as WindowsMetadataExt, OpenOptionsExt},
    io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::mpsc;
use uuid::Uuid;
use walkdir::WalkDir;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FileDispositionInfoEx, GetFileInformationByHandle, GetFinalPathNameByHandleW,
    SetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY,
    FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_WRITE_THROUGH, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

const SKIA_TAG: &str = "m124-08a5439a6b";
const SOURCE_SPACE_MARGIN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 120_000;
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const BUILD_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);
const BUILD_OUTPUT_CHANNEL_CAPACITY: usize = 256;
const BUILD_LOG_LINE_LIMIT_BYTES: usize = 64 * 1024;
const BUILD_OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(target_os = "windows")]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
#[cfg(target_os = "windows")]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

#[cfg(target_os = "linux")]
const SKIA_ASSET: VerifiedAsset<'static> = VerifiedAsset {
    name: "Skia-Linux-Release-x64.zip",
    url: "https://github.com/aseprite/skia/releases/download/m124-08a5439a6b/Skia-Linux-Release-x64.zip",
    size: 35_293_958,
    sha256: "a327e89b244f24cecaa34eb37544bae00d447b96c583d26ed29d6a3ad2e8a8b8",
};

#[cfg(target_os = "windows")]
const SKIA_ASSET: VerifiedAsset<'static> = VerifiedAsset {
    name: "Skia-Windows-Release-x64.zip",
    url: "https://github.com/aseprite/skia/releases/download/m124-08a5439a6b/Skia-Windows-Release-x64.zip",
    size: 28_759_398,
    sha256: "5a371a4b2819bb4eb96e36cd75fa623585e1d5477e253a970302b6f2471b6934",
};

#[derive(Debug, Clone, Copy)]
struct VerifiedAsset<'a> {
    name: &'a str,
    url: &'a str,
    size: u64,
    sha256: &'a str,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OwnershipMarker {
    schema_version: u32,
    installation_id: String,
    source_tag: String,
    source_digest: String,
    #[serde(default)]
    transaction_nonce: Option<String>,
}

fn manual_adoption_required(had_existing: bool, has_managed_record: bool) -> bool {
    had_existing && !has_managed_record
}

fn record_integration_paths(record: &ManagedRecord) -> Vec<PathBuf> {
    record.integration_paths.iter().map(PathBuf::from).collect()
}

fn integration_path_sets_equal(left: &[PathBuf], right: &[PathBuf]) -> bool {
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort();
    right.sort();
    left == right
}

fn build_quarantines<I>(transaction_id: &str, entries: I) -> AppResult<Vec<QuarantineEntry>>
where
    I: IntoIterator<Item = (PathBuf, String)>,
{
    let result = expected_quarantines(transaction_id, entries)?;
    for entry in &result {
        if entry.quarantine.exists() || quarantine_proof_path(&entry.quarantine)?.exists() {
            return Err(InstallerError::with_detail(
                "transactionCollision",
                "A reserved transaction quarantine path is already occupied.",
                entry.quarantine.display().to_string(),
            ));
        }
    }
    Ok(result)
}

fn expected_quarantines<I>(transaction_id: &str, entries: I) -> AppResult<Vec<QuarantineEntry>>
where
    I: IntoIterator<Item = (PathBuf, String)>,
{
    let mut result = Vec::new();
    let mut unique = BTreeSet::new();
    for (source, fingerprint) in entries {
        if !unique.insert(source.clone()) {
            continue;
        }
        let parent = source.parent().ok_or_else(|| {
            InstallerError::with_detail(
                "transactionPath",
                "A transaction cleanup path has no parent directory.",
                source.display().to_string(),
            )
        })?;
        let quarantine = parent.join(format!(
            ".aseprite-quarantine-{transaction_id}-{:02}",
            result.len()
        ));
        result.push(QuarantineEntry {
            source,
            quarantine,
            fingerprint,
        });
    }
    Ok(result)
}

fn integration_snapshot_paths(snapshots: &[IntegrationSnapshot]) -> Vec<PathBuf> {
    snapshots
        .iter()
        .map(|snapshot| snapshot.path.clone())
        .collect()
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
        "Checking this computer…",
    );
    ensure_not_cancelled(&cancelled)?;
    ensure_supported_target(target, existing.is_some())?;
    ensure_aseprite_is_closed(target)?;
    if !portable_source_supported(&release.source_asset_name) {
        return Err(unsupported_source_error());
    }
    let requirements = source_build_requirements(&release.source_asset_name)
        .ok_or_else(unsupported_source_error)?;
    let source_version = requirements.source_version.to_owned();

    let operation_id = Uuid::new_v4().to_string();
    let work_dir = state
        .paths
        .builds_dir
        .join(format!("{}-{operation_id}", release.tag));
    std::fs::create_dir_all(&work_dir)?;
    std::fs::create_dir_all(&state.paths.logs_dir)?;
    let log_path = state.paths.logs_dir.join(format!(
        "{}-{}.log",
        Utc::now().format("%Y%m%d-%H%M%S"),
        release.tag
    ));
    let mut log_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&log_path)?;

    let result = async {
        let source_asset = VerifiedAsset {
            name: &release.source_asset_name,
            url: &release.source_url,
            size: release.size,
            sha256: release.digest.strip_prefix("sha256:").ok_or_else(|| {
                InstallerError::new(
                    "sourceDigest",
                    "The selected source archive does not have a SHA-256 digest.",
                )
            })?,
        };
        validate_source_asset(&source_asset)?;
        let source_archive = download_verified_asset(
            state,
            source_asset,
            &cancelled,
            progress,
            (5, 25),
            "official Aseprite source archive",
        )
        .await?;
        ensure_not_cancelled(&cancelled)?;

        send_stage(
            progress,
            OperationStage::Extracting,
            Some(29),
            "Extracting the verified source archive…",
        );
        let source_extract = work_dir.join("source");
        extract_in_background(
            source_archive,
            source_extract.clone(),
            source_asset.size,
            source_asset.sha256.to_owned(),
            cancelled.clone(),
        )
        .await?;
        let source_root = find_source_root(&source_extract)?;
        validate_declared_skia_tag(&source_root)?;

        let skia_archive = download_verified_asset(
            state,
            SKIA_ASSET,
            &cancelled,
            progress,
            (31, 43),
            "pinned official Skia archive",
        )
        .await?;
        ensure_not_cancelled(&cancelled)?;
        send_stage(
            progress,
            OperationStage::Extracting,
            Some(44),
            "Extracting and validating the pinned Skia toolchain…",
        );
        let skia_extract = work_dir.join("skia");
        extract_in_background(
            skia_archive,
            skia_extract.clone(),
            SKIA_ASSET.size,
            SKIA_ASSET.sha256.to_owned(),
            cancelled.clone(),
        )
        .await?;
        let skia_root = find_skia_root(&skia_extract)?;
        validate_skia_tree(&skia_root)?;

        ensure_not_cancelled(&cancelled)?;
        let build_dir = work_dir.join("build");
        let environment = native::prepare_build_environment(requirements.minimum_cmake_version)?;
        let configure_arguments =
            native::cmake_arguments(&source_root, &build_dir, &skia_root, &environment);
        send_stage(
            progress,
            OperationStage::Compiling,
            Some(48),
            "Configuring the verified Aseprite source tree…",
        );
        let mut configure = Command::new(&environment.cmake);
        configure.args(&configure_arguments);
        environment.configure(&mut configure);
        run_streaming_command(
            configure,
            &cancelled,
            progress,
            &mut log_file,
            &log_path,
            "CMake configuration",
        )
        .await?;

        ensure_not_cancelled(&cancelled)?;
        let jobs = build_parallelism();
        let mut build = Command::new(&environment.cmake);
        build
            .arg("--build")
            .arg(&build_dir)
            .arg("--target")
            .arg("aseprite")
            .arg("--parallel")
            .arg(jobs.to_string());
        environment.configure(&mut build);
        send_stage(
            progress,
            OperationStage::Compiling,
            None,
            "Compiling Aseprite from official sources…",
        );
        run_streaming_command(
            build,
            &cancelled,
            progress,
            &mut log_file,
            &log_path,
            "Aseprite compilation",
        )
        .await?;

        ensure_not_cancelled(&cancelled)?;
        let built = native::find_built_artifact(&build_dir)?;
        validate_complete_artifact(&built, &source_version)?;
        send_stage(
            progress,
            OperationStage::PreparingArtifact,
            Some(78),
            "Preparing and fingerprinting the local Aseprite artifact…",
        );
        let destination = target
            .parent()
            .ok_or_else(|| InstallerError::new("target", "The target path has no parent."))?;
        let staging = destination.join(format!(".aseprite-staging-{operation_id}"));
        ensure_copy_capacity(
            &built,
            destination,
            "destinationSpace",
            "There is not enough free space to stage the verified Aseprite build on the destination volume.",
        )?;
        let mut staging_cleanup = CleanupDirectory::new(staging.clone());
        copy_tree(&built, &staging, &cancelled)?;
        let installation_id = native::installation_id(&target.to_string_lossy());
        write_ownership_marker(&staging, &installation_id, release, &operation_id)?;
        validate_complete_artifact(&staging, &source_version)?;
        let fingerprint = native::artifact_fingerprint(&staging)?;

        ensure_not_cancelled(&cancelled)?;
        ensure_aseprite_is_closed(target)?;
        let committed = commit_installation(
            state,
            release,
            &source_version,
            target,
            existing,
            &staging,
            &installation_id,
            &operation_id,
            fingerprint,
            progress,
        );
        if committed.is_ok() {
            staging_cleanup.disarm();
        }
        committed
    }
    .await;

    match &result {
        Ok(_) => {
            let _ = std::fs::remove_dir_all(&work_dir);
            prune_files(&state.paths.archives_dir, 6);
            prune_files(&state.paths.logs_dir, 10);
        }
        Err(error) if error.code == "cancelled" => {
            let _ = std::fs::remove_dir_all(&work_dir);
            send_stage(
                progress,
                OperationStage::Cancelled,
                None,
                "Operation cancelled. The active installation was not changed.",
            );
        }
        Err(_) => {}
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn commit_installation(
    state: &AppState,
    release: &ReleaseInfo,
    source_version: &str,
    target: &Path,
    existing: Option<&InstallationInfo>,
    staging: &Path,
    installation_id: &str,
    transaction_id: &str,
    fingerprint: [u8; 32],
    progress: &Channel<OperationProgress>,
) -> AppResult<InstallationInfo> {
    let _registry_lock = state.lock_registry()?;
    let mut managed = state.load_managed_state()?;
    ensure_supported_target(target, existing.is_some())?;
    if native::target_aseprite_running(target)? {
        return Err(InstallerError::new(
            "asepriteRunning",
            "Close Aseprite before replacing this installation.",
        ));
    }

    let existing_record = managed
        .installations
        .iter()
        .find(|record| record.id == installation_id)
        .cloned();
    if let Some(record) = &existing_record {
        ensure_record_matches_target(record, target)?;
    }
    // A managed record keeps a single rollback point. Resolve and verify the old
    // one before the transaction so an update can retire it only after the new
    // state has committed. Invalid or user-modified rollback copies are left
    // untouched rather than making an otherwise safe update destructive.
    let superseded_backup = existing_record.as_ref().and_then(|record| {
        match validated_record_backup_path(state, record) {
            Ok(path) => path,
            Err(error) => {
                eprintln!(
                    "Aseprite Installer will preserve an unverified superseded rollback copy: {error}"
                );
                None
            }
        }
    });

    Uuid::parse_str(transaction_id).map_err(|error| {
        InstallerError::with_detail(
            "transactionId",
            "The installation transaction identifier is invalid.",
            error.to_string(),
        )
    })?;
    let transaction_id = transaction_id.to_owned();
    let previous = target.with_file_name(format!(".aseprite-previous-{transaction_id}"));
    let backup = state.paths.backups_dir.join(format!(
        "{}-{}",
        installation_id
            .trim_start_matches("linux-")
            .trim_start_matches("windows-"),
        Utc::now().format("%Y%m%d%H%M%S")
    ));
    let backup_staging = state.paths.backups_dir.join(format!(
        ".{}-staging-{transaction_id}",
        installation_id
            .trim_start_matches("linux-")
            .trim_start_matches("windows-")
    ));
    std::fs::create_dir_all(&state.paths.backups_dir)?;
    ensure_real_transaction_parent(&state.paths.backups_dir)?;
    if previous.exists() || backup.exists() || backup_staging.exists() {
        return Err(InstallerError::new(
            "transactionCollision",
            "A unique rollback path could not be reserved.",
        ));
    }

    send_stage(
        progress,
        OperationStage::BackingUp,
        Some(82),
        "Creating a rollback point…",
    );
    let had_existing = target.exists();
    let adopted_version = if manual_adoption_required(had_existing, existing_record.is_some()) {
        if !existing.is_some_and(|installation| {
            matches!(&installation.channel, InstallationChannel::Manual)
        }) {
            return Err(InstallerError::new(
                "managedOwnership",
                "An existing target without a matching managed record can only be changed through explicit manual-copy adoption.",
            ));
        }
        Some(native::artifact_version(target)?)
    } else {
        None
    };
    let before_integration_paths = existing_record
        .as_ref()
        .map(record_integration_paths)
        .unwrap_or_default();
    let _backup_staging_cleanup = CleanupDirectory::new(backup_staging.clone());
    let backup_fingerprint = if had_existing {
        ensure_copy_capacity(
            target,
            &state.paths.backups_dir,
            "backupSpace",
            "There is not enough free space to preserve the current Aseprite installation on the backup volume.",
        )?;
        let source_fingerprint = native::artifact_fingerprint(target)?;
        let never_cancel = AtomicBool::new(false);
        copy_tree(target, &backup_staging, &never_cancel)?;
        let copied_fingerprint = native::artifact_fingerprint(&backup_staging)?;
        if copied_fingerprint != source_fingerprint {
            return Err(InstallerError::new(
                "backupValidation",
                "The current installation could not be copied exactly to the rollback volume. The active installation was not changed.",
            ));
        }
        Some(copied_fingerprint)
    } else {
        None
    };
    if let Some(expected) = backup_fingerprint {
        if native::artifact_fingerprint(target)? != expected {
            return Err(InstallerError::new(
                "targetChanged",
                "The existing installation changed while its rollback copy was being prepared. The active installation was not changed.",
            ));
        }
        if native::target_aseprite_running(target)? {
            return Err(InstallerError::new(
                "asepriteRunning",
                "Aseprite was opened while its rollback copy was being prepared. Close it and try again.",
            ));
        }
    }
    let integration_plan = native::desktop_integration_paths(installation_id)?;
    native::validate_desktop_integration_paths(&before_integration_paths, installation_id)?;
    native::validate_desktop_integration_paths(&integration_plan, installation_id)?;
    if !before_integration_paths.is_empty()
        && !integration_path_sets_equal(&before_integration_paths, &integration_plan)
    {
        return Err(InstallerError::new(
            "desktopIntegrationProfileChanged",
            "The per-user desktop integration root changed. Restore the original user profile paths before updating this managed installation.",
        ));
    }
    let before_integration =
        native::capture_desktop_integration(&integration_plan, installation_id)?;
    let old_fingerprint_hex = backup_fingerprint.map(hex::encode);
    let new_fingerprint_hex = hex::encode(fingerprint);
    sync_tree_durable(staging)?;
    if had_existing {
        sync_tree_durable(&backup_staging)?;
    }
    let mut quarantine_sources = vec![
        (target.to_path_buf(), new_fingerprint_hex.clone()),
        (staging.to_path_buf(), new_fingerprint_hex.clone()),
    ];
    if let Some(old_fingerprint) = old_fingerprint_hex.as_ref() {
        quarantine_sources.push((previous.clone(), old_fingerprint.clone()));
        quarantine_sources.push((backup_staging.clone(), old_fingerprint.clone()));
        quarantine_sources.push((backup.clone(), old_fingerprint.clone()));
    }
    if let (Some(path), Some(fingerprint)) = (
        superseded_backup.as_ref(),
        existing_record
            .as_ref()
            .and_then(|record| record.backup_bundle_fingerprint.as_ref()),
    ) {
        quarantine_sources.push((path.clone(), fingerprint.clone()));
    }
    let quarantines = build_quarantines(&transaction_id, quarantine_sources)?;
    let mut journal = PortableJournal {
        schema_version: PORTABLE_JOURNAL_SCHEMA_VERSION,
        transaction_id: transaction_id.clone(),
        installation_id: installation_id.into(),
        quarantine_nonce: Uuid::new_v4().to_string(),
        phase: JournalPhase::Prepared,
        before_state_sha256: managed_state_sha256(&managed)?,
        after_state_sha256: None,
        before_record: existing_record.clone(),
        after_record: None,
        quarantines,
        before_integration: before_integration.clone(),
        after_integration: before_integration.clone(),
        operation: JournalOperation::Install(InstallJournal {
            target: target.to_path_buf(),
            staging: staging.to_path_buf(),
            previous: previous.clone(),
            backup_staging: had_existing.then_some(backup_staging.clone()),
            backup: had_existing.then_some(backup.clone()),
            superseded_backup: superseded_backup.clone(),
            old_fingerprint: old_fingerprint_hex.clone(),
            new_fingerprint: new_fingerprint_hex.clone(),
            before_integration_paths: before_integration_paths.clone(),
            after_integration_paths: integration_plan.clone(),
        }),
    };
    write_journal(&state.paths, &journal)?;
    if had_existing {
        durable_rename_no_replace(target, &previous).map_err(|error| {
            InstallerError::with_detail(
                "backup",
                "The existing installation could not be moved to a rollback point.",
                error.to_string(),
            )
        })?;
        advance_journal(state, &mut journal, JournalPhase::TargetPreserved)?;
        let moved_matches = backup_fingerprint
            .is_some_and(|expected| native::artifact_fingerprint(&previous).ok() == Some(expected));
        let moved_running = native::target_aseprite_running(&previous).unwrap_or(true);
        if !moved_matches || moved_running {
            let rollback = durable_rename_no_replace(&previous, target);
            return Err(match rollback {
                Ok(()) => InstallerError::with_detail(
                    if moved_running {
                        "asepriteRunning"
                    } else {
                        "targetChanged"
                    },
                    "The existing installation changed or started running at the atomic replacement boundary.",
                    "The active installation was restored without being replaced.",
                ),
                Err(error) => InstallerError::with_detail(
                    "rollbackFailed",
                    "The existing installation changed at the replacement boundary and could not be moved back automatically.",
                    format!(
                        "The preserved installation remains at {}: {error}",
                        previous.display()
                    ),
                ),
            });
        }
    }

    send_stage(
        progress,
        OperationStage::Installing,
        Some(87),
        "Installing the verified local build atomically…",
    );
    if let Err(error) = durable_rename_no_replace(staging, target) {
        let rollback = if had_existing {
            durable_rename_no_replace(&previous, target).map_err(|error| error.to_string())
        } else {
            Ok(())
        };
        return Err(match rollback {
            Ok(()) => InstallerError::with_detail(
                "installCommit",
                "The new build could not be activated.",
                error.to_string(),
            ),
            Err(rollback) => InstallerError::with_detail(
                "rollbackFailed",
                "The new build could not be activated and the previous installation could not be returned to its target.",
                format!(
                    "Activation error: {error}; rollback error: {rollback}. The recovery journal was preserved."
                ),
            ),
        });
    }
    advance_journal(state, &mut journal, JournalPhase::TargetActivated)?;

    if let Err(error) = validate_complete_artifact(target, source_version) {
        return Err(rollback_new_target(
            &journal,
            target,
            had_existing.then_some(previous.as_path()),
            &new_fingerprint_hex,
            error,
        ));
    }

    send_stage(
        progress,
        OperationStage::Integrating,
        Some(91),
        "Registering the per-user application launcher…",
    );
    let desired_integration =
        native::prepare_desktop_integration(target, installation_id, &integration_plan)?;
    journal.after_integration = desired_integration.clone();
    // Persist both complete file sides before the first launcher/icon mutation.
    advance_journal(state, &mut journal, JournalPhase::TargetActivated)?;
    let integration_paths = match native::apply_desktop_integration(
        &desired_integration,
        &before_integration,
        installation_id,
    ) {
        Ok(paths) => paths,
        Err(error) => {
            return Err(rollback_new_target_with_integration(
                &journal,
                target,
                had_existing.then_some(previous.as_path()),
                &new_fingerprint_hex,
                &desired_integration,
                &before_integration,
                installation_id,
                error,
            ));
        }
    };
    let mut actual_integration_paths = integration_paths.clone();
    let mut expected_integration_paths = integration_plan.clone();
    actual_integration_paths.sort();
    expected_integration_paths.sort();
    if actual_integration_paths != expected_integration_paths {
        return Err(rollback_new_target_with_integration(
            &journal,
            target,
            had_existing.then_some(previous.as_path()),
            &new_fingerprint_hex,
            &desired_integration,
            &before_integration,
            installation_id,
            InstallerError::new(
                "desktopIntegrationPath",
                "Desktop integration was created at unexpected paths.",
            ),
        ));
    }
    advance_journal(state, &mut journal, JournalPhase::IntegrationApplied)?;

    let backup_metadata = if had_existing {
        let backup_fingerprint = backup_fingerprint.ok_or_else(|| {
            InstallerError::new(
                "backupValidation",
                "The rollback copy was not fingerprinted before the active installation changed.",
            )
        })?;
        if let Err(error) = durable_rename_no_replace(&backup_staging, &backup) {
            return Err(rollback_new_target_with_integration(
                &journal,
                target,
                Some(previous.as_path()),
                &new_fingerprint_hex,
                &desired_integration,
                &before_integration,
                installation_id,
                InstallerError::with_detail(
                    "backupCommit",
                    "The rollback copy could not be committed safely.",
                    error.to_string(),
                ),
            ));
        }
        advance_journal(state, &mut journal, JournalPhase::BackupActivated)?;
        Some((
            backup.clone(),
            existing
                .and_then(|installation| installation.version.clone())
                .or_else(|| existing_record.as_ref().map(|record| record.tag.clone()))
                .or_else(|| adopted_version.clone()),
            existing_record
                .as_ref()
                .and_then(|record| record.source_version.clone())
                .or_else(|| adopted_version.clone()),
            existing_record.as_ref().map(|record| record.digest.clone()),
            existing_record
                .as_ref()
                .map(|record| record.installed_at.clone()),
            adopted_version
                .as_ref()
                .map(|_| true)
                .or_else(|| existing.map(|installation| installation.version_exact)),
            hex::encode(backup_fingerprint),
            existing
                .and_then(|installation| installation.architecture.clone())
                .or_else(|| Some("x86_64".into())),
        ))
    } else {
        None
    };

    send_stage(
        progress,
        OperationStage::Validating,
        Some(97),
        "Validating the committed installation and ownership record…",
    );
    if let Err(error) = ensure_installed_fingerprint(target, &fingerprint) {
        if backup_metadata.is_some() {
            let _ = durable_rename_no_replace(&backup, &backup_staging);
        }
        return Err(rollback_new_target_with_integration(
            &journal,
            target,
            had_existing.then_some(previous.as_path()),
            &new_fingerprint_hex,
            &desired_integration,
            &before_integration,
            installation_id,
            error,
        ));
    }

    let installed_at = Utc::now().to_rfc3339();
    let record = ManagedRecord {
        id: installation_id.into(),
        path: target.to_string_lossy().into_owned(),
        tag: release.tag.clone(),
        source_version: Some(source_version.into()),
        version_exact: true,
        digest: release.digest.clone(),
        architecture: "x86_64".into(),
        installed_at: installed_at.clone(),
        bundle_fingerprint: Some(new_fingerprint_hex.clone()),
        backup_path: backup_metadata
            .as_ref()
            .map(|metadata| metadata.0.to_string_lossy().into_owned()),
        backup_tag: backup_metadata
            .as_ref()
            .and_then(|metadata| metadata.1.clone()),
        backup_source_version: backup_metadata
            .as_ref()
            .and_then(|metadata| metadata.2.clone()),
        backup_digest: backup_metadata
            .as_ref()
            .and_then(|metadata| metadata.3.clone()),
        backup_installed_at: backup_metadata
            .as_ref()
            .and_then(|metadata| metadata.4.clone()),
        backup_version_exact: backup_metadata.as_ref().and_then(|metadata| metadata.5),
        backup_bundle_fingerprint: backup_metadata.as_ref().map(|metadata| metadata.6.clone()),
        backup_architecture: backup_metadata
            .as_ref()
            .and_then(|metadata| metadata.7.clone()),
        integration_paths: integration_paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    };
    managed
        .installations
        .retain(|candidate| candidate.id != installation_id);
    managed.installations.push(record);
    managed.schema_version = MANAGED_STATE_SCHEMA_VERSION;
    journal.after_state_sha256 = Some(managed_state_sha256(&managed)?);
    journal.after_record = managed
        .installations
        .iter()
        .find(|record| record.id == installation_id)
        .cloned();
    advance_journal(state, &mut journal, JournalPhase::CommitReady)?;
    let registry_durable = match state.save_managed_state_transactional(&managed) {
        Ok(CommitDurability::Durable) => true,
        Ok(CommitDurability::Uncertain(detail)) => {
            eprintln!(
                "Aseprite Installer committed the managed state with uncertain persistence; preserving the transaction journal and both filesystem sides: {detail}"
            );
            false
        }
        Err(error) => {
            if backup_metadata.is_some() {
                let _ = durable_rename_no_replace(&backup, &backup_staging);
            }
            return Err(rollback_new_target_with_integration(
                &journal,
                target,
                had_existing.then_some(previous.as_path()),
                &new_fingerprint_hex,
                &desired_integration,
                &before_integration,
                installation_id,
                error,
            ));
        }
    };

    if registry_durable {
        journal.phase = JournalPhase::RegistryCommitted;
        if let Err(error) = write_journal(&state.paths, &journal) {
            eprintln!(
                "Aseprite Installer committed the managed state but could not mark its transaction journal committed: {error}"
            );
        }

        let mut cleanup_complete = true;
        if previous.exists() {
            if let Err(error) = remove_verified_transaction_directory(
                &journal,
                &previous,
                old_fingerprint_hex.as_deref().unwrap_or_default(),
            ) {
                cleanup_complete = false;
                eprintln!("Could not clean the committed previous transaction copy: {error}");
            }
        }
        if let (Some(superseded_backup), Some(superseded_fingerprint)) = (
            superseded_backup.filter(|path| path != &backup),
            existing_record
                .as_ref()
                .and_then(|record| record.backup_bundle_fingerprint.as_deref()),
        ) {
            if let Err(error) = remove_verified_transaction_directory(
                &journal,
                &superseded_backup,
                superseded_fingerprint,
            ) {
                cleanup_complete = false;
                eprintln!(
                    "Aseprite Installer committed the update but could not move the superseded verified rollback copy {} to the system trash: {error}",
                    superseded_backup.display()
                );
            }
        }
        if cleanup_complete {
            if let Err(error) = remove_journal(&state.paths) {
                eprintln!("Could not remove the completed transaction journal: {error}");
            }
        }
    }
    send_stage(
        progress,
        OperationStage::Completed,
        Some(100),
        "Aseprite is ready.",
    );
    Ok(InstallationInfo {
        id: installation_id.into(),
        path: target.to_string_lossy().into_owned(),
        version: Some(source_version.into()),
        version_exact: true,
        architecture: Some("x86_64".into()),
        channel: InstallationChannel::Managed,
        manageable: true,
        writable: true,
        has_backup: backup_metadata.is_some(),
        installed_at: Some(installed_at),
    })
}

fn rollback_new_target(
    journal: &PortableJournal,
    target: &Path,
    previous: Option<&Path>,
    expected_active_fingerprint: &str,
    original: InstallerError,
) -> InstallerError {
    let rollback = rollback_directory_replacement(
        journal,
        target,
        previous,
        Some(expected_active_fingerprint),
    );
    match rollback {
        Ok(()) => original,
        Err(rollback) => InstallerError::with_detail(
            "rollbackFailed",
            "The install failed and the previous installation could not be restored automatically.",
            format!("Original error: {original}; rollback error: {rollback}"),
        ),
    }
}

fn rollback_directory_replacement(
    journal: &PortableJournal,
    active: &Path,
    previous: Option<&Path>,
    expected_active_fingerprint: Option<&str>,
) -> Result<(), String> {
    if let Some(previous) = previous {
        if !previous.exists() {
            return Err(format!(
                "The preserved transaction copy is missing: {}",
                previous.display()
            ));
        }
    }
    if active.exists() {
        let expected = expected_active_fingerprint.ok_or_else(|| {
            format!(
                "The active artifact was preserved because no verified fingerprint was available: {}",
                active.display()
            )
        })?;
        remove_verified_transaction_directory(journal, active, expected)
            .map_err(|error| error.to_string())?;
    }
    if let Some(previous) = previous {
        if previous.exists() {
            durable_rename_no_replace(previous, active).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn activate_staged_directory(
    active: &Path,
    staging: &Path,
    previous: &Path,
    code: &str,
    message: &str,
) -> AppResult<()> {
    if previous.exists() {
        return Err(InstallerError::with_detail(
            "transactionCollision",
            "A restore transaction path is already occupied.",
            previous.display().to_string(),
        ));
    }
    durable_rename_no_replace(active, previous).map_err(|error| {
        InstallerError::with_detail(code, message, format!("{}: {error}", active.display()))
    })?;
    if let Err(error) = durable_rename_no_replace(staging, active) {
        let rollback = durable_rename_no_replace(previous, active);
        return Err(match rollback {
            Ok(()) => InstallerError::with_detail(code, message, error.to_string()),
            Err(rollback) => InstallerError::with_detail(
                "rollbackFailed",
                "A transaction activation failed and its preserved item could not be returned automatically.",
                format!(
                    "Activation error: {error}; rollback error: {rollback}. The preserved item remains at {}.",
                    previous.display()
                ),
            ),
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn rollback_new_target_with_integration(
    journal: &PortableJournal,
    target: &Path,
    previous: Option<&Path>,
    expected_active_fingerprint: &str,
    new_integration: &[IntegrationSnapshot],
    previous_integration: &[IntegrationSnapshot],
    installation_id: &str,
    original: InstallerError,
) -> InstallerError {
    let integration_restore =
        native::apply_desktop_integration(previous_integration, new_integration, installation_id)
            .map(|_| ())
            .map_err(|error| error.to_string());
    let filesystem_rollback = rollback_directory_replacement(
        journal,
        target,
        previous,
        Some(expected_active_fingerprint),
    );

    if filesystem_rollback.is_ok() && integration_restore.is_ok() {
        original
    } else {
        InstallerError::with_detail(
            "rollbackFailed",
            "The install failed and its filesystem or desktop integration could not be restored completely.",
            format!(
                "Original error: {original}; filesystem rollback: {}; integration restore: {}",
                filesystem_rollback
                    .err()
                    .unwrap_or_else(|| "succeeded".into()),
                integration_restore
                    .err()
                    .unwrap_or_else(|| "succeeded".into()),
            ),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn rollback_restore_transaction(
    journal: &PortableJournal,
    target: &Path,
    target_previous: &Path,
    backup: &Path,
    backup_previous: &Path,
    target_active_fingerprint: &str,
    backup_active_fingerprint: &str,
    new_integration: &[IntegrationSnapshot],
    previous_integration: &[IntegrationSnapshot],
    installation_id: &str,
    original: InstallerError,
) -> InstallerError {
    let integration_restore =
        native::apply_desktop_integration(previous_integration, new_integration, installation_id)
            .map(|_| ())
            .map_err(|error| error.to_string());
    let backup_rollback = rollback_directory_replacement(
        journal,
        backup,
        Some(backup_previous),
        Some(backup_active_fingerprint),
    );
    let target_rollback = rollback_directory_replacement(
        journal,
        target,
        Some(target_previous),
        Some(target_active_fingerprint),
    );
    if backup_rollback.is_ok() && target_rollback.is_ok() && integration_restore.is_ok() {
        original
    } else {
        InstallerError::with_detail(
            "rollbackFailed",
            "The restore failed and its filesystem or desktop integration could not be returned to the original state completely.",
            format!(
                "Original error: {original}; backup rollback: {}; target rollback: {}; integration restore: {}",
                backup_rollback
                    .err()
                    .unwrap_or_else(|| "succeeded".into()),
                target_rollback
                    .err()
                    .unwrap_or_else(|| "succeeded".into()),
                integration_restore
                    .err()
                    .unwrap_or_else(|| "succeeded".into()),
            ),
        )
    }
}

pub fn restore_previous(state: &AppState, id: &str) -> AppResult<InstallationInfo> {
    let _registry_lock = state.lock_registry()?;
    let mut managed = state.load_managed_state()?;
    let index = managed
        .installations
        .iter()
        .position(|record| record.id == id)
        .ok_or_else(|| {
            InstallerError::new("notFound", "The managed installation was not found.")
        })?;
    let record = managed.installations[index].clone();
    let target = PathBuf::from(&record.path);
    let backup = validated_record_backup_path(state, &record)?
        .ok_or_else(|| InstallerError::new("noBackup", "No previous installation is available."))?;
    validate_record_identity(&record, &target)?;
    ensure_record_fingerprint(
        &target,
        record.bundle_fingerprint.as_deref(),
        "current installation",
    )?;
    ensure_record_fingerprint(
        &backup,
        record.backup_bundle_fingerprint.as_deref(),
        "rollback copy",
    )?;
    ensure_aseprite_is_closed(&target)?;
    let restored_version = record
        .backup_source_version
        .clone()
        .or_else(|| record.backup_tag.clone())
        .ok_or_else(|| {
            InstallerError::new(
                "backupMetadata",
                "The rollback copy does not have a recorded source version.",
            )
        })?;

    let transaction_id = Uuid::new_v4().to_string();
    let target_staging =
        target.with_file_name(format!(".aseprite-restore-staging-{transaction_id}"));
    let target_previous =
        target.with_file_name(format!(".aseprite-restore-current-{transaction_id}"));
    let backup_parent = backup.parent().ok_or_else(|| {
        InstallerError::new("backupPath", "The rollback copy has no parent directory.")
    })?;
    ensure_real_transaction_parent(backup_parent)?;
    let backup_staging =
        backup_parent.join(format!(".aseprite-restore-backup-staging-{transaction_id}"));
    let backup_previous =
        backup_parent.join(format!(".aseprite-restore-previous-{transaction_id}"));
    for reserved in [
        &target_staging,
        &target_previous,
        &backup_staging,
        &backup_previous,
    ] {
        if reserved.exists() {
            return Err(InstallerError::with_detail(
                "transactionCollision",
                "A restore transaction path is already occupied.",
                reserved.display().to_string(),
            ));
        }
    }

    let target_parent = target.parent().ok_or_else(|| {
        InstallerError::new("targetPath", "The managed target has no parent directory.")
    })?;
    ensure_copy_capacity(
        &backup,
        target_parent,
        "restoreDestinationSpace",
        "There is not enough free space to stage the rollback copy on the destination volume.",
    )?;
    let target_staging_cleanup = CleanupDirectory::new(target_staging.clone());
    let never_cancel = AtomicBool::new(false);
    copy_tree(&backup, &target_staging, &never_cancel)?;
    ensure_record_fingerprint(
        &target_staging,
        record.backup_bundle_fingerprint.as_deref(),
        "staged rollback copy",
    )?;
    validate_complete_artifact(&target_staging, &restored_version)?;
    ensure_backup_ownership_marker(
        &target_staging,
        &record.id,
        &restored_version,
        record
            .backup_digest
            .as_deref()
            .unwrap_or("unrecorded-manual-copy"),
        &transaction_id,
    )?;
    let restored_fingerprint = native::artifact_fingerprint(&target_staging)?;
    let restored_fingerprint_hex = hex::encode(restored_fingerprint);

    ensure_copy_capacity(
        &target,
        backup_parent,
        "restoreBackupSpace",
        "There is not enough free space to preserve the current installation on the backup volume.",
    )?;
    let backup_staging_cleanup = CleanupDirectory::new(backup_staging.clone());
    copy_tree(&target, &backup_staging, &never_cancel)?;
    ensure_record_fingerprint(
        &backup_staging,
        record.bundle_fingerprint.as_deref(),
        "staged current installation",
    )?;
    ensure_record_fingerprint(
        &target,
        record.bundle_fingerprint.as_deref(),
        "current installation before restore",
    )?;
    ensure_record_fingerprint(
        &backup,
        record.backup_bundle_fingerprint.as_deref(),
        "rollback copy before restore",
    )?;
    ensure_aseprite_is_closed(&target)?;

    let current_fingerprint = record.bundle_fingerprint.clone().ok_or_else(|| {
        InstallerError::new(
            "missingFingerprint",
            "The current installation does not have a portable ownership fingerprint.",
        )
    })?;
    ensure_real_transaction_parent(target_parent)?;
    ensure_real_transaction_parent(&state.paths.backups_dir)?;
    let rollback_fingerprint = record.backup_bundle_fingerprint.clone().ok_or_else(|| {
        InstallerError::new(
            "missingFingerprint",
            "The rollback copy does not have a portable ownership fingerprint.",
        )
    })?;
    let before_integration_paths = record_integration_paths(&record);
    let integration_plan = native::desktop_integration_paths(&record.id)?;
    native::validate_desktop_integration_paths(&before_integration_paths, &record.id)?;
    native::validate_desktop_integration_paths(&integration_plan, &record.id)?;
    if !integration_path_sets_equal(&before_integration_paths, &integration_plan) {
        return Err(InstallerError::new(
            "desktopIntegrationProfileChanged",
            "The per-user desktop integration root changed. Restore the original user profile paths before restoring this managed installation.",
        ));
    }
    let before_integration = native::capture_desktop_integration(&integration_plan, &record.id)?;
    sync_tree_durable(&target_staging)?;
    sync_tree_durable(&backup_staging)?;
    let quarantines = build_quarantines(
        &transaction_id,
        [
            (target.clone(), restored_fingerprint_hex.clone()),
            (target_staging.clone(), restored_fingerprint_hex.clone()),
            (target_previous.clone(), current_fingerprint.clone()),
            (backup.clone(), current_fingerprint.clone()),
            (backup_staging.clone(), current_fingerprint.clone()),
            (backup_previous.clone(), rollback_fingerprint.clone()),
        ],
    )?;
    let mut journal = PortableJournal {
        schema_version: PORTABLE_JOURNAL_SCHEMA_VERSION,
        transaction_id: transaction_id.clone(),
        installation_id: record.id.clone(),
        quarantine_nonce: Uuid::new_v4().to_string(),
        phase: JournalPhase::Prepared,
        before_state_sha256: managed_state_sha256(&managed)?,
        after_state_sha256: None,
        before_record: Some(record.clone()),
        after_record: None,
        quarantines,
        before_integration: before_integration.clone(),
        after_integration: before_integration.clone(),
        operation: JournalOperation::Restore(RestoreJournal {
            target: target.clone(),
            target_staging: target_staging.clone(),
            target_previous: target_previous.clone(),
            backup: backup.clone(),
            backup_staging: backup_staging.clone(),
            backup_previous: backup_previous.clone(),
            current_fingerprint: current_fingerprint.clone(),
            rollback_fingerprint: rollback_fingerprint.clone(),
            restored_fingerprint: restored_fingerprint_hex.clone(),
            before_integration_paths: before_integration_paths.clone(),
            after_integration_paths: integration_plan.clone(),
        }),
    };
    write_journal(&state.paths, &journal)?;

    activate_staged_directory(
        &target,
        &target_staging,
        &target_previous,
        "restoreActivation",
        "The rollback copy could not be activated on the destination volume.",
    )?;
    advance_journal(state, &mut journal, JournalPhase::TargetActivated)?;
    let current_boundary_ok = ensure_record_fingerprint(
        &target_previous,
        record.bundle_fingerprint.as_deref(),
        "preserved current installation",
    )
    .is_ok()
        && !native::target_aseprite_running(&target_previous).unwrap_or(true);
    if !current_boundary_ok {
        return Err(rollback_new_target(
            &journal,
            &target,
            Some(&target_previous),
            &restored_fingerprint_hex,
            InstallerError::new(
                "restoreBoundary",
                "The current installation changed or started running at the restore boundary.",
            ),
        ));
    }
    if let Err(error) = activate_staged_directory(
        &backup,
        &backup_staging,
        &backup_previous,
        "restoreBackupActivation",
        "The current installation could not be activated as the new rollback copy.",
    ) {
        let backup_repair = if backup.exists() {
            Ok(())
        } else if backup_previous.exists() {
            durable_rename_no_replace(&backup_previous, &backup).map_err(|error| error.to_string())
        } else {
            Err("The preserved rollback copy is missing.".into())
        };
        let target_repair = rollback_directory_replacement(
            &journal,
            &target,
            Some(&target_previous),
            Some(&restored_fingerprint_hex),
        );
        if backup_repair.is_ok() && target_repair.is_ok() {
            return Err(error);
        }
        return Err(InstallerError::with_detail(
            "rollbackFailed",
            "The restore could not activate both transaction sides and could not return them to their original paths completely.",
            format!(
                "Activation error: {error}; target repair: {}; backup repair: {}",
                target_repair.err().unwrap_or_else(|| "succeeded".into()),
                backup_repair.err().unwrap_or_else(|| "succeeded".into()),
            ),
        ));
    }
    advance_journal(state, &mut journal, JournalPhase::BackupActivated)?;

    if ensure_record_fingerprint(
        &backup_previous,
        record.backup_bundle_fingerprint.as_deref(),
        "preserved rollback copy",
    )
    .is_err()
    {
        return Err(rollback_restore_transaction(
            &journal,
            &target,
            &target_previous,
            &backup,
            &backup_previous,
            &restored_fingerprint_hex,
            &current_fingerprint,
            &before_integration,
            &before_integration,
            &record.id,
            InstallerError::new(
                "restoreBoundary",
                "The rollback copy changed at the atomic restore boundary.",
            ),
        ));
    }
    if let Err(error) = ensure_installed_fingerprint(&target, &restored_fingerprint) {
        return Err(rollback_restore_transaction(
            &journal,
            &target,
            &target_previous,
            &backup,
            &backup_previous,
            &restored_fingerprint_hex,
            &current_fingerprint,
            &before_integration,
            &before_integration,
            &record.id,
            error,
        ));
    }
    if let Err(error) = ensure_record_fingerprint(
        &backup,
        record.bundle_fingerprint.as_deref(),
        "new rollback copy",
    ) {
        return Err(rollback_restore_transaction(
            &journal,
            &target,
            &target_previous,
            &backup,
            &backup_previous,
            &restored_fingerprint_hex,
            &current_fingerprint,
            &before_integration,
            &before_integration,
            &record.id,
            error,
        ));
    }
    let desired_integration =
        native::prepare_desktop_integration(&target, &record.id, &integration_plan)?;
    journal.after_integration = desired_integration.clone();
    advance_journal(state, &mut journal, JournalPhase::BackupActivated)?;
    let integration_paths = match native::apply_desktop_integration(
        &desired_integration,
        &before_integration,
        &record.id,
    ) {
        Ok(paths) => paths,
        Err(error) => {
            return Err(rollback_restore_transaction(
                &journal,
                &target,
                &target_previous,
                &backup,
                &backup_previous,
                &restored_fingerprint_hex,
                &current_fingerprint,
                &desired_integration,
                &before_integration,
                &record.id,
                error,
            ));
        }
    };
    let mut actual_integration_paths = integration_paths.clone();
    let mut expected_integration_paths = integration_plan.clone();
    actual_integration_paths.sort();
    expected_integration_paths.sort();
    if actual_integration_paths != expected_integration_paths {
        return Err(rollback_restore_transaction(
            &journal,
            &target,
            &target_previous,
            &backup,
            &backup_previous,
            &restored_fingerprint_hex,
            &current_fingerprint,
            &desired_integration,
            &before_integration,
            &record.id,
            InstallerError::new(
                "desktopIntegrationPath",
                "Desktop integration was created at unexpected paths.",
            ),
        ));
    }
    advance_journal(state, &mut journal, JournalPhase::IntegrationApplied)?;

    let current = managed.installations[index].clone();
    let restored = &mut managed.installations[index];
    restored.tag = current
        .backup_tag
        .clone()
        .unwrap_or_else(|| restored_version.clone());
    restored.source_version = current.backup_source_version.clone();
    restored.digest = current.backup_digest.clone().unwrap_or_default();
    restored.installed_at = current
        .backup_installed_at
        .clone()
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    restored.version_exact = current.backup_version_exact.unwrap_or(false);
    restored.bundle_fingerprint = Some(hex::encode(restored_fingerprint));
    restored.architecture = current
        .backup_architecture
        .clone()
        .unwrap_or_else(|| "x86_64".into());
    restored.backup_tag = Some(current.tag);
    restored.backup_source_version = current.source_version;
    restored.backup_digest = Some(current.digest);
    restored.backup_installed_at = Some(current.installed_at);
    restored.backup_version_exact = Some(current.version_exact);
    restored.backup_bundle_fingerprint = current.bundle_fingerprint;
    restored.backup_architecture = Some(current.architecture);
    restored.integration_paths = integration_paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    managed.schema_version = MANAGED_STATE_SCHEMA_VERSION;
    journal.after_state_sha256 = Some(managed_state_sha256(&managed)?);
    journal.after_record = Some(managed.installations[index].clone());
    advance_journal(state, &mut journal, JournalPhase::CommitReady)?;
    let registry_durable = match state.save_managed_state_transactional(&managed) {
        Ok(CommitDurability::Durable) => true,
        Ok(CommitDurability::Uncertain(detail)) => {
            eprintln!(
                "Aseprite Installer committed the restore state with uncertain persistence; preserving both restore sides and the journal: {detail}"
            );
            false
        }
        Err(error) => {
            return Err(rollback_restore_transaction(
                &journal,
                &target,
                &target_previous,
                &backup,
                &backup_previous,
                &restored_fingerprint_hex,
                &current_fingerprint,
                &desired_integration,
                &before_integration,
                &record.id,
                error,
            ));
        }
    };
    drop(target_staging_cleanup);
    drop(backup_staging_cleanup);
    if registry_durable {
        journal.phase = JournalPhase::RegistryCommitted;
        if let Err(error) = write_journal(&state.paths, &journal) {
            eprintln!(
                "Aseprite Installer committed the restore state but could not mark its transaction journal committed: {error}"
            );
        }

        let mut cleanup_complete = true;
        if let Err(error) = remove_verified_transaction_directory(
            &journal,
            &target_previous,
            &record.bundle_fingerprint.clone().unwrap_or_default(),
        ) {
            cleanup_complete = false;
            eprintln!("Could not clean the committed restore target copy: {error}");
        }
        if let Err(error) = remove_verified_transaction_directory(
            &journal,
            &backup_previous,
            &record.backup_bundle_fingerprint.clone().unwrap_or_default(),
        ) {
            cleanup_complete = false;
            eprintln!("Could not clean the committed restore backup copy: {error}");
        }
        if cleanup_complete {
            if let Err(error) = remove_journal(&state.paths) {
                eprintln!("Could not remove the completed restore journal: {error}");
            }
        }
    }

    let record = &managed.installations[index];
    Ok(InstallationInfo {
        id: record.id.clone(),
        path: record.path.clone(),
        version: record
            .source_version
            .clone()
            .or_else(|| Some(record.tag.clone())),
        version_exact: record.version_exact,
        architecture: Some(record.architecture.clone()),
        channel: InstallationChannel::Managed,
        manageable: true,
        writable: true,
        has_backup: true,
        installed_at: Some(record.installed_at.clone()),
    })
}

pub fn uninstall_managed(state: &AppState, id: &str) -> AppResult<()> {
    let _registry_lock = state.lock_registry()?;
    let mut managed = state.load_managed_state()?;
    let index = managed
        .installations
        .iter()
        .position(|record| record.id == id)
        .ok_or_else(|| {
            InstallerError::new("notFound", "The managed installation was not found.")
        })?;
    let record = managed.installations[index].clone();
    let target = PathBuf::from(&record.path);
    validate_record_identity(&record, &target)?;
    ensure_record_fingerprint(
        &target,
        record.bundle_fingerprint.as_deref(),
        "managed installation",
    )?;
    let backup_to_delete = validated_record_backup_path(state, &record)?;
    ensure_aseprite_is_closed(&target)?;
    let integration_paths = record_integration_paths(&record);
    native::validate_desktop_integration_paths(&integration_paths, &record.id)?;
    let before_integration = native::capture_desktop_integration(&integration_paths, &record.id)?;
    let after_integration = native::absent_desktop_integration(&integration_paths, &record.id)?;
    let mut committed_state = managed.clone();
    committed_state.installations.remove(index);
    committed_state.schema_version = MANAGED_STATE_SCHEMA_VERSION;
    let transaction_id = Uuid::new_v4().to_string();
    let target_fingerprint = record.bundle_fingerprint.clone().ok_or_else(|| {
        InstallerError::new(
            "missingFingerprint",
            "The managed installation has no portable ownership fingerprint.",
        )
    })?;
    let mut quarantine_sources = vec![(target.clone(), target_fingerprint.clone())];
    if let (Some(backup), Some(fingerprint)) = (
        backup_to_delete.as_ref(),
        record.backup_bundle_fingerprint.as_ref(),
    ) {
        quarantine_sources.push((backup.clone(), fingerprint.clone()));
    }
    let quarantines = build_quarantines(&transaction_id, quarantine_sources)?;
    let mut journal = PortableJournal {
        schema_version: PORTABLE_JOURNAL_SCHEMA_VERSION,
        transaction_id: transaction_id.clone(),
        installation_id: record.id.clone(),
        quarantine_nonce: Uuid::new_v4().to_string(),
        phase: JournalPhase::Prepared,
        before_state_sha256: managed_state_sha256(&managed)?,
        after_state_sha256: Some(managed_state_sha256(&committed_state)?),
        before_record: Some(record.clone()),
        after_record: None,
        quarantines,
        before_integration: before_integration.clone(),
        after_integration: after_integration.clone(),
        operation: JournalOperation::Uninstall(UninstallJournal {
            target: target.clone(),
            target_fingerprint: target_fingerprint.clone(),
            backup: backup_to_delete.clone(),
            backup_fingerprint: record.backup_bundle_fingerprint.clone(),
            before_integration_paths: integration_paths.clone(),
            after_integration_paths: Vec::new(),
        }),
    };
    write_journal(&state.paths, &journal)?;
    if let Err(error) =
        native::apply_desktop_integration(&after_integration, &before_integration, &record.id)
    {
        let restoration =
            native::apply_desktop_integration(&before_integration, &after_integration, &record.id);
        return Err(match restoration {
            Ok(_) => error,
            Err(restoration) => InstallerError::with_detail(
                "rollbackFailed",
                "Desktop integration could not be removed and then restored completely.",
                format!("Removal error: {error}; restoration error: {restoration}"),
            ),
        });
    }
    advance_journal(state, &mut journal, JournalPhase::IntegrationApplied)?;
    quarantine_verified_directory(&journal, &target, &target_fingerprint)?;
    if let (Some(backup), Some(fingerprint)) = (
        backup_to_delete.as_ref(),
        record.backup_bundle_fingerprint.as_deref(),
    ) {
        quarantine_verified_directory(&journal, backup, fingerprint)?;
    }
    advance_journal(state, &mut journal, JournalPhase::CommitReady)?;
    managed = committed_state;
    match state.save_managed_state_transactional(&managed) {
        Ok(CommitDurability::Durable) => {}
        Ok(CommitDurability::Uncertain(detail)) => {
            eprintln!(
                "Aseprite Installer committed the uninstall state with uncertain persistence; preserving the verified files and journal for recovery: {detail}"
            );
            return Ok(());
        }
        Err(error) => {
            let recovery = recover_pending_transaction_locked(state);
            return Err(match recovery {
                Ok(()) => error,
                Err(recovery) => InstallerError::with_detail(
                    "rollbackFailed",
                    "The uninstall registry commit failed and its verified files could not be restored completely.",
                    format!("Commit error: {error}; recovery error: {recovery}"),
                ),
            });
        }
    }

    journal.phase = JournalPhase::RegistryCommitted;
    if let Err(error) = write_journal(&state.paths, &journal) {
        eprintln!(
            "Aseprite Installer committed the uninstall state but could not mark its transaction journal committed: {error}"
        );
    }
    remove_quarantined_directory(&journal, &target, &target_fingerprint)?;
    if let (Some(backup), Some(fingerprint)) = (
        backup_to_delete.as_ref(),
        record.backup_bundle_fingerprint.as_deref(),
    ) {
        remove_quarantined_directory(&journal, backup, fingerprint)?;
    }
    if let Err(error) = remove_journal(&state.paths) {
        eprintln!("Could not remove the completed uninstall journal: {error}");
    }
    Ok(())
}

fn validated_record_backup_path(
    state: &AppState,
    record: &ManagedRecord,
) -> AppResult<Option<PathBuf>> {
    let Some(stored) = record.backup_path.as_deref() else {
        return Ok(None);
    };
    let backup = PathBuf::from(stored);
    let metadata = match std::fs::symlink_metadata(&backup) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(InstallerError::with_detail(
            "backupPath",
            "The recorded rollback copy is not a real directory and will not be removed.",
            backup.display().to_string(),
        ));
    }
    let backup_parent = backup.parent().ok_or_else(|| {
        InstallerError::new(
            "backupPath",
            "The recorded rollback copy has no parent directory.",
        )
    })?;
    let allowed_parent = std::fs::canonicalize(&state.paths.backups_dir)?;
    let actual_parent = std::fs::canonicalize(backup_parent)?;
    if actual_parent != allowed_parent {
        return Err(InstallerError::with_detail(
            "backupPath",
            "The recorded rollback copy is outside Aseprite Installer's backup directory and will not be removed.",
            backup.display().to_string(),
        ));
    }
    ensure_record_fingerprint(
        &backup,
        record.backup_bundle_fingerprint.as_deref(),
        "rollback copy",
    )?;
    let marker_path = backup.join(".aseprite-installer.json");
    if marker_path.exists() {
        let marker: OwnershipMarker = serde_json::from_slice(&std::fs::read(&marker_path)?)
            .map_err(|error| {
                InstallerError::with_detail(
                    "ownershipMarker",
                    "The rollback copy ownership marker is invalid.",
                    error.to_string(),
                )
            })?;
        if marker.installation_id != record.id {
            return Err(InstallerError::new(
                "ownershipMarker",
                "The rollback copy belongs to another managed installation and will not be removed.",
            ));
        }
    }
    Ok(Some(backup))
}

fn recovery_conflict(message: &str, detail: impl Into<String>) -> InstallerError {
    InstallerError::with_detail("recoveryConflict", message, detail.into())
}

fn advance_journal(
    state: &AppState,
    journal: &mut PortableJournal,
    phase: JournalPhase,
) -> AppResult<()> {
    journal.phase = phase;
    if let Err(write_error) = write_journal(&state.paths, journal) {
        // Once the initial journal is durable every phase transition follows a
        // filesystem mutation. Never return with only the in-memory phase: use
        // the registry fingerprint (not the stale phase) to synchronously drive
        // the durable journal back to a complete side.
        let recovery = recover_pending_transaction_locked(state);
        return Err(match recovery {
            Ok(()) => InstallerError::with_detail(
                "transactionJournal",
                "A transaction journal update failed after a mutation; the original state was recovered synchronously.",
                write_error.to_string(),
            ),
            Err(recovery_error) => InstallerError::with_detail(
                "rollbackFailed",
                "A transaction journal update failed after a mutation and synchronous recovery did not complete.",
                format!("Journal error: {write_error}; recovery error: {recovery_error}"),
            ),
        });
    }
    Ok(())
}

fn artifact_fingerprint_if_present(path: &Path) -> AppResult<Option<String>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&metadata)
        || !metadata.is_dir()
    {
        return Err(recovery_conflict(
            "A transaction artifact changed type and cannot be recovered automatically.",
            path.display().to_string(),
        ));
    }
    Ok(Some(hex::encode(native::artifact_fingerprint(path)?)))
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuarantineProof {
    transaction_id: String,
    quarantine_nonce: String,
    installation_id: String,
    source: PathBuf,
    quarantine: PathBuf,
    fingerprint: String,
}

fn quarantine_proof_path(quarantine: &Path) -> AppResult<PathBuf> {
    let name = quarantine
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            recovery_conflict(
                "A transaction quarantine has no valid file name.",
                quarantine.display().to_string(),
            )
        })?;
    Ok(quarantine.with_file_name(format!("{name}.proof")))
}

fn quarantine_entry<'a>(
    journal: &'a PortableJournal,
    source: &Path,
    expected: &str,
) -> AppResult<&'a QuarantineEntry> {
    let mut matches = journal
        .quarantines
        .iter()
        .filter(|entry| entry.source == source && entry.fingerprint == expected);
    let entry = matches.next().ok_or_else(|| {
        recovery_conflict(
            "A cleanup path is not reserved by the active transaction journal.",
            source.display().to_string(),
        )
    })?;
    if matches.next().is_some() {
        return Err(recovery_conflict(
            "A cleanup path is duplicated in the active transaction journal.",
            source.display().to_string(),
        ));
    }
    Ok(entry)
}

fn expected_quarantine_proof(
    journal: &PortableJournal,
    entry: &QuarantineEntry,
) -> QuarantineProof {
    QuarantineProof {
        transaction_id: journal.transaction_id.clone(),
        quarantine_nonce: journal.quarantine_nonce.clone(),
        installation_id: journal.installation_id.clone(),
        source: entry.source.clone(),
        quarantine: entry.quarantine.clone(),
        fingerprint: entry.fingerprint.clone(),
    }
}

fn read_quarantine_proof(
    journal: &PortableJournal,
    entry: &QuarantineEntry,
) -> AppResult<Option<QuarantineProof>> {
    let path = quarantine_proof_path(&entry.quarantine)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    #[cfg(target_os = "windows")]
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata_is_reparse_point(&metadata) || metadata.len() > 16 * 1024 {
        return Err(recovery_conflict(
            "A transaction quarantine proof changed type or size.",
            path.display().to_string(),
        ));
    }
    let mut encoded = Vec::with_capacity(metadata.len() as usize);
    file.take(16 * 1024 + 1).read_to_end(&mut encoded)?;
    let proof: QuarantineProof = serde_json::from_slice(&encoded).map_err(|error| {
        recovery_conflict(
            "A transaction quarantine proof is invalid.",
            format!("{}: {error}", path.display()),
        )
    })?;
    let expected = expected_quarantine_proof(journal, entry);
    if proof.transaction_id != expected.transaction_id
        || proof.quarantine_nonce != expected.quarantine_nonce
        || proof.installation_id != expected.installation_id
        || proof.source != expected.source
        || proof.quarantine != expected.quarantine
        || proof.fingerprint != expected.fingerprint
    {
        return Err(recovery_conflict(
            "A transaction quarantine proof does not match its journal reservation.",
            path.display().to_string(),
        ));
    }
    Ok(Some(proof))
}

fn write_quarantine_proof(journal: &PortableJournal, entry: &QuarantineEntry) -> AppResult<()> {
    match read_quarantine_proof(journal, entry) {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {}
        Err(error) => {
            // This function is called only after the renamed tree has been
            // re-fingerprinted. A torn proof write is therefore safe to retire;
            // a non-regular/reparse proof is still preserved as a conflict.
            let proof_path = quarantine_proof_path(&entry.quarantine)?;
            let metadata = std::fs::symlink_metadata(&proof_path)?;
            if !metadata.is_file() || metadata_is_reparse_point(&metadata) {
                return Err(error);
            }
            std::fs::remove_file(&proof_path)?;
        }
    }
    let path = quarantine_proof_path(&entry.quarantine)?;
    let encoded = serde_json::to_vec(&expected_quarantine_proof(journal, entry))?;
    let temporary = path.with_file_name(format!(
        "{}.tmp-{}",
        path.file_name().and_then(OsStr::to_str).unwrap_or("proof"),
        journal.quarantine_nonce
    ));
    match std::fs::symlink_metadata(&temporary) {
        Ok(metadata) if metadata.is_file() && !metadata_is_reparse_point(&metadata) => {
            std::fs::remove_file(&temporary)?;
        }
        Ok(_) => {
            return Err(recovery_conflict(
                "A transaction quarantine proof staging path changed type.",
                temporary.display().to_string(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    #[cfg(target_os = "windows")]
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let mut file = options.open(&temporary)?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    drop(file);
    durable_rename_file_no_replace(&temporary, &path)?;
    read_quarantine_proof(journal, entry)?.ok_or_else(|| {
        recovery_conflict(
            "A transaction quarantine proof was not durable after activation.",
            path.display().to_string(),
        )
    })?;
    Ok(())
}

fn verify_cleanup_ownership(journal: &PortableJournal, path: &Path) -> AppResult<()> {
    let marker_path = path.join(".aseprite-installer.json");
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    #[cfg(target_os = "windows")]
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = match options.open(&marker_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A registry-owned legacy tree is still bound by its exact managed
            // record. Explicit manual adoption can also have an unmarked old
            // side, but only at reserved old-copy/backup roles; the new/fresh
            // target always requires the transaction nonce below.
            if journal.before_record.is_some()
                || matches!(&journal.operation, JournalOperation::Install(operation)
                if operation.old_fingerprint.is_some()
                    && journal.quarantines.iter().any(|entry| {
                        (entry.source == path || entry.quarantine == path)
                            && operation.old_fingerprint.as_deref()
                                == Some(entry.fingerprint.as_str())
                            && entry.source != operation.target
                            && entry.source != operation.staging
                    }))
            {
                return Ok(());
            }
            return Err(recovery_conflict(
                "A transaction artifact has no safely readable ownership marker.",
                format!("{}: {error}", marker_path.display()),
            ));
        }
        Err(error) => {
            return Err(recovery_conflict(
                "A transaction artifact ownership marker could not be opened safely.",
                format!("{}: {error}", marker_path.display()),
            ))
        }
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata_is_reparse_point(&metadata) || metadata.len() > 64 * 1024 {
        return Err(recovery_conflict(
            "A transaction artifact ownership marker changed type or size.",
            marker_path.display().to_string(),
        ));
    }
    let mut encoded = Vec::with_capacity(metadata.len() as usize);
    file.take(64 * 1024 + 1).read_to_end(&mut encoded)?;
    let marker: OwnershipMarker = serde_json::from_slice(&encoded).map_err(|error| {
        recovery_conflict(
            "A transaction artifact ownership marker is invalid.",
            format!("{}: {error}", marker_path.display()),
        )
    })?;
    if marker.installation_id != journal.installation_id {
        return Err(recovery_conflict(
            "A transaction artifact belongs to another managed installation.",
            path.display().to_string(),
        ));
    }
    if journal.before_record.is_none()
        && marker.transaction_nonce.as_deref() != Some(journal.transaction_id.as_str())
    {
        return Err(recovery_conflict(
            "A fresh-install artifact is not bound to this transaction nonce.",
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn quarantine_verified_directory(
    journal: &PortableJournal,
    path: &Path,
    expected: &str,
) -> AppResult<()> {
    let entry = quarantine_entry(journal, path, expected)?;
    let quarantine_metadata = std::fs::symlink_metadata(&entry.quarantine);
    match quarantine_metadata {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || metadata_is_reparse_point(&metadata)
                || !metadata.is_dir() =>
        {
            return Err(recovery_conflict(
                "A reserved transaction quarantine changed type.",
                entry.quarantine.display().to_string(),
            ))
        }
        Ok(_) => {
            if artifact_fingerprint_if_present(&entry.quarantine)?.as_deref() == Some(expected) {
                verify_cleanup_ownership(journal, &entry.quarantine)?;
                write_quarantine_proof(journal, entry)?;
                return Ok(());
            }
            if read_quarantine_proof(journal, entry)?.is_some() {
                // A valid proof deliberately survives partial recursive cleanup.
                return Ok(());
            }
            return Err(recovery_conflict(
                "A transaction quarantine is incomplete without a matching durable proof.",
                entry.quarantine.display().to_string(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let Some(actual) = artifact_fingerprint_if_present(path)? else {
        return Ok(());
    };
    if actual != expected {
        return Err(recovery_conflict(
            "A transaction artifact changed and will not be quarantined automatically.",
            format!("{}: expected {expected}, found {actual}", path.display()),
        ));
    }
    verify_cleanup_ownership(journal, path)?;
    durable_rename_no_replace(path, &entry.quarantine)?;
    let moved = artifact_fingerprint_if_present(&entry.quarantine)?;
    if moved.as_deref() != Some(expected) {
        return Err(recovery_conflict(
            "A transaction artifact changed across its atomic quarantine rename.",
            entry.quarantine.display().to_string(),
        ));
    }
    verify_cleanup_ownership(journal, &entry.quarantine)?;
    write_quarantine_proof(journal, entry)
}

fn remove_quarantined_directory(
    journal: &PortableJournal,
    source: &Path,
    expected: &str,
) -> AppResult<()> {
    let entry = quarantine_entry(journal, source, expected)?;
    quarantine_verified_directory(journal, source, expected)?;
    let metadata = match std::fs::symlink_metadata(&entry.quarantine) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let proof_path = quarantine_proof_path(&entry.quarantine)?;
            if read_quarantine_proof(journal, entry)?.is_some() {
                std::fs::remove_file(proof_path)?;
            }
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&metadata)
        || !metadata.is_dir()
    {
        return Err(recovery_conflict(
            "A transaction quarantine changed type before cleanup.",
            entry.quarantine.display().to_string(),
        ));
    }
    read_quarantine_proof(journal, entry)?.ok_or_else(|| {
        recovery_conflict(
            "A transaction quarantine has no durable cleanup proof.",
            entry.quarantine.display().to_string(),
        )
    })?;
    remove_real_tree_no_links(&entry.quarantine)?;
    let proof_path = quarantine_proof_path(&entry.quarantine)?;
    match std::fs::remove_file(&proof_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn restore_quarantined_directory(
    journal: &PortableJournal,
    source: &Path,
    expected: &str,
) -> AppResult<()> {
    let entry = quarantine_entry(journal, source, expected)?;
    if let Some(actual) = artifact_fingerprint_if_present(source)? {
        if actual != expected {
            return Err(recovery_conflict(
                "A transaction rollback destination contains an unexpected artifact.",
                source.display().to_string(),
            ));
        }
        verify_cleanup_ownership(journal, source)?;
        if entry.quarantine.exists() {
            return Err(recovery_conflict(
                "Both sides of a transaction quarantine are occupied.",
                entry.quarantine.display().to_string(),
            ));
        }
        return Ok(());
    }
    let actual = artifact_fingerprint_if_present(&entry.quarantine)?.ok_or_else(|| {
        recovery_conflict(
            "The verified transaction artifact needed for rollback is missing.",
            source.display().to_string(),
        )
    })?;
    if actual != expected {
        return Err(recovery_conflict(
            "A transaction quarantine was partially cleaned before rollback.",
            entry.quarantine.display().to_string(),
        ));
    }
    verify_cleanup_ownership(journal, &entry.quarantine)?;
    let proof = quarantine_proof_path(&entry.quarantine)?;
    match std::fs::remove_file(&proof) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    durable_rename_no_replace(&entry.quarantine, source)?;
    if artifact_fingerprint_if_present(source)?.as_deref() != Some(expected) {
        return Err(recovery_conflict(
            "A restored quarantine did not retain its verified fingerprint.",
            source.display().to_string(),
        ));
    }
    verify_cleanup_ownership(journal, source)
}

#[cfg(target_os = "linux")]
fn remove_real_tree_no_links(root: &Path) -> AppResult<()> {
    let parent = root.parent().ok_or_else(|| {
        recovery_conflict(
            "A quarantined cleanup root has no parent directory.",
            root.display().to_string(),
        )
    })?;
    let name = linux_component_name(root.file_name().ok_or_else(|| {
        recovery_conflict(
            "A quarantined cleanup root has no file name.",
            root.display().to_string(),
        )
    })?)?;
    let parent = linux_open_absolute_directory_no_links(parent)?;
    linux_remove_directory_at(parent.as_raw_fd(), &name, root)
}

#[cfg(target_os = "linux")]
fn linux_open_absolute_directory_no_links(path: &Path) -> AppResult<OwnedFd> {
    if !path.is_absolute() {
        return Err(recovery_conflict(
            "A cleanup parent path is not absolute.",
            path.display().to_string(),
        ));
    }
    let slash = CString::new("/").expect("a slash contains no NUL byte");
    let raw = unsafe {
        libc::open(
            slash.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut directory = unsafe { OwnedFd::from_raw_fd(raw) };
    for component in path.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => {
                let name = linux_component_name(name)?;
                directory = linux_open_directory_at(directory.as_raw_fd(), &name)?;
            }
            _ => {
                return Err(recovery_conflict(
                    "A cleanup parent path contains a traversal component.",
                    path.display().to_string(),
                ))
            }
        }
    }
    Ok(directory)
}

#[cfg(target_os = "linux")]
fn linux_component_name(name: &OsStr) -> AppResult<CString> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(recovery_conflict(
            "A cleanup path contains an invalid component.",
            name.to_string_lossy(),
        ));
    }
    CString::new(bytes).map_err(|_| {
        recovery_conflict(
            "A cleanup path contains a NUL byte.",
            name.to_string_lossy(),
        )
    })
}

#[cfg(target_os = "linux")]
fn linux_open_directory_at(parent: RawFd, name: &CStr) -> AppResult<OwnedFd> {
    let raw = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }
}

#[cfg(target_os = "linux")]
fn linux_stat_at(parent: RawFd, name: &CStr) -> AppResult<libc::stat> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::zeroed();
    let result = unsafe {
        libc::fstatat(
            parent,
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(unsafe { metadata.assume_init() })
    }
}

#[cfg(target_os = "linux")]
fn linux_stat_fd(fd: RawFd) -> AppResult<libc::stat> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::zeroed();
    let result = unsafe { libc::fstat(fd, metadata.as_mut_ptr()) };
    if result != 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(unsafe { metadata.assume_init() })
    }
}

#[cfg(target_os = "linux")]
fn linux_same_entry(left: &libc::stat, right: &libc::stat) -> bool {
    left.st_dev == right.st_dev
        && left.st_ino == right.st_ino
        && left.st_mode & libc::S_IFMT == right.st_mode & libc::S_IFMT
}

#[cfg(target_os = "linux")]
fn linux_directory_names(directory: RawFd) -> AppResult<Vec<OsString>> {
    let duplicate = unsafe { libc::dup(directory) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe { libc::close(duplicate) };
        return Err(error.into());
    }
    let result = (|| {
        let mut names = Vec::new();
        loop {
            unsafe { *libc::__errno_location() = 0 };
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error().unwrap_or(0) == 0 {
                    break;
                }
                return Err(error.into());
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            names.push(OsString::from_vec(name.to_vec()));
        }
        names.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        Ok(names)
    })();
    let close_result = unsafe { libc::closedir(stream) };
    if close_result != 0 && result.is_ok() {
        return Err(std::io::Error::last_os_error().into());
    }
    result
}

#[cfg(target_os = "linux")]
fn linux_remove_directory_at(parent: RawFd, name: &CStr, display: &Path) -> AppResult<()> {
    let before = linux_stat_at(parent, name)?;
    if before.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(recovery_conflict(
            "A quarantined cleanup directory changed type.",
            display.display().to_string(),
        ));
    }
    let directory = linux_open_directory_at(parent, name)?;
    let opened = linux_stat_fd(directory.as_raw_fd())?;
    if !linux_same_entry(&before, &opened) {
        return Err(recovery_conflict(
            "A quarantined cleanup directory changed while it was being opened.",
            display.display().to_string(),
        ));
    }

    for child_name in linux_directory_names(directory.as_raw_fd())? {
        let child = linux_component_name(&child_name)?;
        let child_display = display.join(&child_name);
        let before = linux_stat_at(directory.as_raw_fd(), &child)?;
        match before.st_mode & libc::S_IFMT {
            libc::S_IFDIR => {
                linux_remove_directory_at(directory.as_raw_fd(), &child, &child_display)?;
            }
            libc::S_IFREG => {
                let raw = unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        child.as_ptr(),
                        libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if raw < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let opened = unsafe { OwnedFd::from_raw_fd(raw) };
                let opened_stat = linux_stat_fd(opened.as_raw_fd())?;
                let current = linux_stat_at(directory.as_raw_fd(), &child)?;
                if !linux_same_entry(&before, &opened_stat)
                    || !linux_same_entry(&opened_stat, &current)
                {
                    return Err(recovery_conflict(
                        "A quarantined cleanup file changed before deletion.",
                        child_display.display().to_string(),
                    ));
                }
                if unsafe { libc::unlinkat(directory.as_raw_fd(), child.as_ptr(), 0) } != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
            _ => {
                return Err(recovery_conflict(
                    "A quarantined artifact contains a link or special filesystem entry.",
                    child_display.display().to_string(),
                ))
            }
        }
    }
    if unsafe { libc::fsync(directory.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let current = linux_stat_at(parent, name)?;
    if !linux_same_entry(&opened, &current) {
        return Err(recovery_conflict(
            "A quarantined cleanup directory changed before deletion.",
            display.display().to_string(),
        ));
    }
    if unsafe { libc::unlinkat(parent, name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if unsafe { libc::fsync(parent) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn remove_real_tree_no_links(root: &Path) -> AppResult<()> {
    let root_handle = windows_open_cleanup_handle(root, true)?;
    windows_validate_cleanup_handle(&root_handle, root, true)?;
    windows_remove_directory_handle(root, &root_handle)
}

#[cfg(target_os = "windows")]
fn windows_remove_directory_handle(path: &Path, directory: &OwnedHandle) -> AppResult<()> {
    windows_validate_cleanup_handle(directory, path, true)?;
    let mut entries = std::fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let child_name = entry.file_name();
        if child_name.is_empty() || child_name == OsStr::new(".") || child_name == OsStr::new("..")
        {
            return Err(recovery_conflict(
                "A quarantined cleanup path contains an invalid component.",
                path.display().to_string(),
            ));
        }
        let child_path = path.join(&child_name);
        let metadata = std::fs::symlink_metadata(&child_path)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(recovery_conflict(
                "A quarantined artifact contains a reparse point.",
                child_path.display().to_string(),
            ));
        }
        if metadata.is_dir() {
            let child = windows_open_cleanup_handle(&child_path, true)?;
            windows_validate_cleanup_child(directory, &child, &child_name, true)?;
            windows_remove_directory_handle(&child_path, &child)?;
        } else if metadata.is_file() {
            let child = windows_open_cleanup_handle(&child_path, false)?;
            windows_validate_cleanup_child(directory, &child, &child_name, false)?;
            windows_delete_by_handle(&child)?;
        } else {
            return Err(recovery_conflict(
                "A quarantined artifact contains a special filesystem entry.",
                child_path.display().to_string(),
            ));
        }
    }
    windows_delete_by_handle(directory)
}

#[cfg(target_os = "windows")]
fn windows_open_cleanup_handle(path: &Path, directory: bool) -> AppResult<OwnedHandle> {
    let path = windows_wide(path);
    let access = DELETE | FILE_READ_ATTRIBUTES | if directory { FILE_LIST_DIRECTORY } else { 0 };
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) })
    }
}

#[cfg(target_os = "windows")]
fn windows_validate_cleanup_handle(
    handle: &OwnedHandle,
    expected_path: &Path,
    directory: bool,
) -> AppResult<()> {
    windows_validate_cleanup_type(handle, directory)?;
    let actual = windows_normalize_final_path(&windows_cleanup_final_path(handle)?);
    let expected = windows_normalize_final_path(expected_path.as_os_str());
    if actual != expected {
        return Err(recovery_conflict(
            "A quarantined cleanup path resolved through a junction or changed while it was opened.",
            format!("expected {expected}; found {actual}"),
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_validate_cleanup_child(
    parent: &OwnedHandle,
    child: &OwnedHandle,
    child_name: &OsStr,
    directory: bool,
) -> AppResult<()> {
    windows_validate_cleanup_type(child, directory)?;
    let parent = PathBuf::from(windows_cleanup_final_path(parent)?);
    let expected = windows_normalize_final_path(parent.join(child_name).as_os_str());
    let actual = windows_normalize_final_path(&windows_cleanup_final_path(child)?);
    if actual != expected {
        return Err(recovery_conflict(
            "A quarantined cleanup child escaped its opened parent directory.",
            format!("expected {expected}; found {actual}"),
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_validate_cleanup_type(handle: &OwnedHandle, directory: bool) -> AppResult<()> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle.as_raw_handle() as _, &mut information) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let attributes = information.dwFileAttributes;
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (attributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
    {
        return Err(recovery_conflict(
            "A quarantined cleanup handle changed type or is a reparse point.",
            format!("Windows attributes: 0x{attributes:08x}"),
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_delete_by_handle(handle: &OwnedHandle) -> AppResult<()> {
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    if unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle() as _,
            FileDispositionInfoEx,
            std::ptr::from_ref(&disposition).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_cleanup_final_path(handle: &OwnedHandle) -> AppResult<std::ffi::OsString> {
    let required = unsafe {
        GetFinalPathNameByHandleW(handle.as_raw_handle() as _, std::ptr::null_mut(), 0, 0)
    };
    if required == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut path = vec![0_u16; required as usize + 1];
    let written = unsafe {
        GetFinalPathNameByHandleW(
            handle.as_raw_handle() as _,
            path.as_mut_ptr(),
            path.len() as u32,
            0,
        )
    };
    if written == 0 || written as usize >= path.len() {
        return Err(std::io::Error::last_os_error().into());
    }
    path.truncate(written as usize);
    Ok(std::ffi::OsString::from_wide(&path))
}

#[cfg(target_os = "windows")]
fn windows_normalize_final_path(path: &OsStr) -> String {
    let mut normalized = path.to_string_lossy().replace('/', "\\");
    if let Some(rest) = normalized.strip_prefix("\\\\?\\UNC\\") {
        normalized = format!("\\\\{rest}");
    } else if let Some(rest) = normalized.strip_prefix("\\\\?\\") {
        normalized = rest.to_owned();
    }
    while normalized.len() > 3 && normalized.ends_with('\\') {
        normalized.pop();
    }
    normalized.to_lowercase()
}

#[cfg(target_os = "windows")]
fn windows_wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn remove_verified_transaction_directory(
    journal: &PortableJournal,
    path: &Path,
    expected: &str,
) -> AppResult<()> {
    remove_quarantined_directory(journal, path, expected)
}

fn ensure_recovery_artifact(
    journal: &PortableJournal,
    active: &Path,
    desired_fingerprint: &str,
    candidates: &[&Path],
    permitted_active_fingerprint: Option<&str>,
) -> AppResult<()> {
    if artifact_fingerprint_if_present(active)?.as_deref() == Some(desired_fingerprint) {
        return Ok(());
    }
    let mut source = None;
    for candidate in candidates {
        if artifact_fingerprint_if_present(candidate)?.as_deref() == Some(desired_fingerprint) {
            source = Some(*candidate);
            break;
        }
    }
    let source = source.ok_or_else(|| {
            recovery_conflict(
                "The artifact needed to complete transaction recovery is missing.",
                format!(
                    "Expected fingerprint {desired_fingerprint} at {} or one of its reserved transaction paths.",
                    active.display()
                ),
            )
        })?;

    if let Some(actual) = artifact_fingerprint_if_present(active)? {
        if Some(actual.as_str()) != permitted_active_fingerprint {
            return Err(recovery_conflict(
                "The active transaction path contains an unexpected artifact.",
                format!("{}: {actual}", active.display()),
            ));
        }
        remove_verified_transaction_directory(journal, active, &actual)?;
    }
    durable_rename_no_replace(source, active)?;
    if artifact_fingerprint_if_present(active)?.as_deref() != Some(desired_fingerprint) {
        return Err(recovery_conflict(
            "The recovered artifact did not retain its verified fingerprint.",
            active.display().to_string(),
        ));
    }
    Ok(())
}

fn recover_desktop_integration(
    installation_id: &str,
    desired: &[IntegrationSnapshot],
    alternative: &[IntegrationSnapshot],
) -> AppResult<()> {
    native::apply_desktop_integration(desired, alternative, installation_id)?;
    Ok(())
}

pub(crate) fn recover_pending_transaction(state: &AppState) -> AppResult<()> {
    let _registry_lock = state.lock_registry()?;
    recover_pending_transaction_locked(state)
}

fn recover_pending_transaction_locked(state: &AppState) -> AppResult<()> {
    let Some(journal) = load_journal(&state.paths)? else {
        return Ok(());
    };
    validate_recovery_journal(state, &journal)?;
    let managed = state.load_managed_state()?;
    let direction = recovery_direction(&journal, &managed_state_sha256(&managed)?)?;
    let operation = journal.operation.clone();
    let before_record = journal.before_record.clone();
    let after_record = journal.after_record.clone();
    let installation_id = journal.installation_id.clone();

    match (operation, direction) {
        (JournalOperation::Install(operation), RecoveryDirection::BeforeCommit) => {
            recover_install_before(
                &journal,
                &operation,
                &installation_id,
                before_record.as_ref(),
            )?
        }
        (JournalOperation::Install(operation), RecoveryDirection::AfterCommit) => {
            recover_install_after(
                state,
                &journal,
                &operation,
                &installation_id,
                before_record.as_ref(),
                after_record.as_ref().ok_or_else(|| {
                    recovery_conflict(
                        "The committed install transaction has no after-state ownership record.",
                        installation_id.as_str(),
                    )
                })?,
            )?
        }
        (JournalOperation::Restore(operation), RecoveryDirection::BeforeCommit) => {
            recover_restore_before(
                &journal,
                &operation,
                &installation_id,
                before_record.as_ref().ok_or_else(|| {
                    recovery_conflict(
                        "The restore transaction has no before-state ownership record.",
                        installation_id.as_str(),
                    )
                })?,
            )?
        }
        (JournalOperation::Restore(operation), RecoveryDirection::AfterCommit) => {
            recover_restore_after(
                &journal,
                &operation,
                &installation_id,
                after_record.as_ref().ok_or_else(|| {
                    recovery_conflict(
                        "The committed restore transaction has no after-state ownership record.",
                        installation_id.as_str(),
                    )
                })?,
            )?
        }
        (JournalOperation::Uninstall(operation), RecoveryDirection::BeforeCommit) => {
            recover_uninstall_before(
                state,
                &journal,
                &operation,
                &installation_id,
                before_record.as_ref().ok_or_else(|| {
                    recovery_conflict(
                        "The uninstall transaction has no before-state ownership record.",
                        installation_id.as_str(),
                    )
                })?,
            )?
        }
        (JournalOperation::Uninstall(operation), RecoveryDirection::AfterCommit) => {
            recover_uninstall_after(
                state,
                &journal,
                &operation,
                &installation_id,
                before_record.as_ref().ok_or_else(|| {
                    recovery_conflict(
                        "The uninstall transaction has no ownership record for verified cleanup.",
                        installation_id.as_str(),
                    )
                })?,
            )?
        }
    }
    remove_journal(&state.paths)
}

fn recover_install_before(
    journal: &PortableJournal,
    operation: &InstallJournal,
    installation_id: &str,
    _before_record: Option<&ManagedRecord>,
) -> AppResult<()> {
    if let Some(old_fingerprint) = operation.old_fingerprint.as_deref() {
        ensure_recovery_artifact(
            journal,
            &operation.target,
            old_fingerprint,
            &[&operation.previous],
            Some(&operation.new_fingerprint),
        )?;
        remove_verified_transaction_directory(journal, &operation.previous, old_fingerprint)?;
    } else if let Some(actual) = artifact_fingerprint_if_present(&operation.target)? {
        if actual != operation.new_fingerprint {
            return Err(recovery_conflict(
                "A fresh-install target contains an unexpected artifact.",
                operation.target.display().to_string(),
            ));
        }
        remove_verified_transaction_directory(
            journal,
            &operation.target,
            &operation.new_fingerprint,
        )?;
    }
    remove_verified_transaction_directory(journal, &operation.staging, &operation.new_fingerprint)?;
    if let Some(old_fingerprint) = operation.old_fingerprint.as_deref() {
        if let Some(path) = operation.backup_staging.as_deref() {
            remove_verified_transaction_directory(journal, path, old_fingerprint)?;
        }
        if let Some(path) = operation.backup.as_deref() {
            remove_verified_transaction_directory(journal, path, old_fingerprint)?;
        }
    }
    recover_desktop_integration(
        installation_id,
        &journal.before_integration,
        &journal.after_integration,
    )
}

fn recover_install_after(
    state: &AppState,
    journal: &PortableJournal,
    operation: &InstallJournal,
    installation_id: &str,
    before_record: Option<&ManagedRecord>,
    after_record: &ManagedRecord,
) -> AppResult<()> {
    ensure_recovery_artifact(
        journal,
        &operation.target,
        &operation.new_fingerprint,
        &[&operation.staging],
        operation.old_fingerprint.as_deref(),
    )?;
    if let Some(backup) = operation.backup.as_deref() {
        let old_fingerprint = operation.old_fingerprint.as_deref().ok_or_else(|| {
            recovery_conflict(
                "A committed update journal has a backup path but no old fingerprint.",
                backup.display().to_string(),
            )
        })?;
        let staging = operation.backup_staging.as_deref().ok_or_else(|| {
            recovery_conflict(
                "A committed update journal has no backup staging path.",
                backup.display().to_string(),
            )
        })?;
        ensure_recovery_artifact(journal, backup, old_fingerprint, &[staging], None)?;
        remove_verified_transaction_directory(journal, staging, old_fingerprint)?;
    }
    ensure_record_matches_target(after_record, &operation.target)?;
    if let Some(expected_backup) = operation.backup.as_deref() {
        if validated_record_backup_path(state, after_record)?.as_deref() != Some(expected_backup) {
            return Err(recovery_conflict(
                "The committed rollback path does not match its ownership record.",
                expected_backup.display().to_string(),
            ));
        }
    }
    recover_desktop_integration(
        installation_id,
        &journal.after_integration,
        &journal.before_integration,
    )?;
    if let Some(old_fingerprint) = operation.old_fingerprint.as_deref() {
        remove_verified_transaction_directory(journal, &operation.previous, old_fingerprint)?;
    }
    remove_verified_transaction_directory(journal, &operation.staging, &operation.new_fingerprint)?;
    if let (Some(superseded), Some(before_record)) =
        (operation.superseded_backup.as_deref(), before_record)
    {
        match validated_record_backup_path(state, before_record) {
            Ok(Some(verified)) if verified == superseded => {
                if let Some(fingerprint) = before_record.backup_bundle_fingerprint.as_deref() {
                    if let Err(error) = remove_verified_transaction_directory(
                        journal,
                        superseded,
                        fingerprint,
                    ) {
                    eprintln!(
                        "Could not retire a superseded verified rollback copy during recovery: {error}"
                    );
                    }
                }
            }
            Ok(None) => {}
            Ok(Some(_)) | Err(_) => eprintln!(
                "Preserving a superseded rollback copy whose recovery ownership could not be verified: {}",
                superseded.display()
            ),
        }
    }
    Ok(())
}

fn recover_restore_before(
    journal: &PortableJournal,
    operation: &RestoreJournal,
    installation_id: &str,
    _before_record: &ManagedRecord,
) -> AppResult<()> {
    ensure_recovery_artifact(
        journal,
        &operation.target,
        &operation.current_fingerprint,
        &[&operation.target_previous],
        Some(&operation.restored_fingerprint),
    )?;
    ensure_recovery_artifact(
        journal,
        &operation.backup,
        &operation.rollback_fingerprint,
        &[&operation.backup_previous],
        Some(&operation.current_fingerprint),
    )?;
    recover_desktop_integration(
        installation_id,
        &journal.before_integration,
        &journal.after_integration,
    )?;
    remove_verified_transaction_directory(
        journal,
        &operation.target_staging,
        &operation.restored_fingerprint,
    )?;
    remove_verified_transaction_directory(
        journal,
        &operation.target_previous,
        &operation.current_fingerprint,
    )?;
    remove_verified_transaction_directory(
        journal,
        &operation.backup_staging,
        &operation.current_fingerprint,
    )?;
    remove_verified_transaction_directory(
        journal,
        &operation.backup_previous,
        &operation.rollback_fingerprint,
    )
}

fn recover_restore_after(
    journal: &PortableJournal,
    operation: &RestoreJournal,
    installation_id: &str,
    after_record: &ManagedRecord,
) -> AppResult<()> {
    ensure_recovery_artifact(
        journal,
        &operation.target,
        &operation.restored_fingerprint,
        &[&operation.target_staging],
        Some(&operation.current_fingerprint),
    )?;
    ensure_recovery_artifact(
        journal,
        &operation.backup,
        &operation.current_fingerprint,
        &[&operation.backup_staging],
        Some(&operation.rollback_fingerprint),
    )?;
    ensure_record_matches_target(after_record, &operation.target)?;
    recover_desktop_integration(
        installation_id,
        &journal.after_integration,
        &journal.before_integration,
    )?;
    remove_verified_transaction_directory(
        journal,
        &operation.target_previous,
        &operation.current_fingerprint,
    )?;
    remove_verified_transaction_directory(
        journal,
        &operation.target_staging,
        &operation.restored_fingerprint,
    )?;
    remove_verified_transaction_directory(
        journal,
        &operation.backup_previous,
        &operation.rollback_fingerprint,
    )?;
    remove_verified_transaction_directory(
        journal,
        &operation.backup_staging,
        &operation.current_fingerprint,
    )
}

fn recover_uninstall_before(
    state: &AppState,
    journal: &PortableJournal,
    operation: &UninstallJournal,
    installation_id: &str,
    before_record: &ManagedRecord,
) -> AppResult<()> {
    restore_quarantined_directory(journal, &operation.target, &operation.target_fingerprint)?;
    if let (Some(backup), Some(fingerprint)) = (
        operation.backup.as_deref(),
        operation.backup_fingerprint.as_deref(),
    ) {
        restore_quarantined_directory(journal, backup, fingerprint)?;
    }
    ensure_record_matches_target(before_record, &operation.target)?;
    if let Some(expected_backup) = operation.backup.as_deref() {
        if validated_record_backup_path(state, before_record)?.as_deref() != Some(expected_backup) {
            return Err(recovery_conflict(
                "The uninstall rollback path no longer matches its ownership record.",
                expected_backup.display().to_string(),
            ));
        }
    }
    recover_desktop_integration(
        installation_id,
        &journal.before_integration,
        &journal.after_integration,
    )
}

fn recover_uninstall_after(
    _state: &AppState,
    journal: &PortableJournal,
    operation: &UninstallJournal,
    installation_id: &str,
    _before_record: &ManagedRecord,
) -> AppResult<()> {
    recover_desktop_integration(
        installation_id,
        &journal.after_integration,
        &journal.before_integration,
    )?;
    remove_verified_transaction_directory(
        journal,
        &operation.target,
        &operation.target_fingerprint,
    )?;
    if let (Some(backup), Some(fingerprint)) = (
        operation.backup.as_deref(),
        operation.backup_fingerprint.as_deref(),
    ) {
        remove_verified_transaction_directory(journal, backup, fingerprint)?;
    }
    Ok(())
}

fn validate_recovery_journal(state: &AppState, journal: &PortableJournal) -> AppResult<()> {
    let target = match &journal.operation {
        JournalOperation::Install(operation) => &operation.target,
        JournalOperation::Restore(operation) => &operation.target,
        JournalOperation::Uninstall(operation) => &operation.target,
    };
    if !target.is_absolute()
        || native::installation_id(&target.to_string_lossy()) != journal.installation_id
    {
        return Err(recovery_conflict(
            "The transaction target does not match its installation identity.",
            target.display().to_string(),
        ));
    }
    for record in [
        journal.before_record.as_ref(),
        journal.after_record.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if record.id != journal.installation_id || Path::new(&record.path) != target {
            return Err(recovery_conflict(
                "A transaction ownership record does not match its target.",
                record.id.clone(),
            ));
        }
    }
    let (before_integration_paths, after_integration_paths) = match &journal.operation {
        JournalOperation::Install(operation) => (
            &operation.before_integration_paths,
            &operation.after_integration_paths,
        ),
        JournalOperation::Restore(operation) => (
            &operation.before_integration_paths,
            &operation.after_integration_paths,
        ),
        JournalOperation::Uninstall(operation) => (
            &operation.before_integration_paths,
            &operation.after_integration_paths,
        ),
    };
    native::validate_desktop_integration_paths(before_integration_paths, &journal.installation_id)?;
    native::validate_desktop_integration_paths(after_integration_paths, &journal.installation_id)?;
    let snapshot_paths = if after_integration_paths.is_empty() {
        before_integration_paths
    } else {
        after_integration_paths
    };
    for snapshots in [&journal.before_integration, &journal.after_integration] {
        if !integration_path_sets_equal(&integration_snapshot_paths(snapshots), snapshot_paths) {
            return Err(recovery_conflict(
                "Desktop integration snapshot paths do not match the transaction plan.",
                journal.installation_id.clone(),
            ));
        }
    }
    let before_should_exist = !before_integration_paths.is_empty();
    if journal
        .before_integration
        .iter()
        .any(|snapshot| snapshot.contents.is_some() != before_should_exist)
    {
        return Err(recovery_conflict(
            "Desktop integration before-snapshots do not match the ownership record.",
            journal.installation_id.clone(),
        ));
    }
    let expected_before_paths = journal
        .before_record
        .as_ref()
        .map(record_integration_paths)
        .unwrap_or_default();
    if !integration_path_sets_equal(before_integration_paths, &expected_before_paths) {
        return Err(recovery_conflict(
            "The transaction before-state desktop integration does not match its ownership record.",
            journal.installation_id.clone(),
        ));
    }
    if let Some(after_record) = journal.after_record.as_ref() {
        let expected_after_paths = record_integration_paths(after_record);
        if !integration_path_sets_equal(after_integration_paths, &expected_after_paths) {
            return Err(recovery_conflict(
                "The transaction after-state desktop integration does not match its ownership record.",
                journal.installation_id.clone(),
            ));
        }
    }
    let target_parent = target.parent().ok_or_else(|| {
        recovery_conflict(
            "The transaction target has no parent directory.",
            target.display().to_string(),
        )
    })?;
    match &journal.operation {
        JournalOperation::Install(operation) => {
            validate_exact_reserved_child(
                &operation.staging,
                target_parent,
                &format!(".aseprite-staging-{}", journal.transaction_id),
            )?;
            validate_exact_reserved_child(
                &operation.previous,
                target_parent,
                &format!(".aseprite-previous-{}", journal.transaction_id),
            )?;
            validate_backup_children(
                state,
                [operation.backup_staging.as_ref(), operation.backup.as_ref()],
            )?;
            if let Some(path) = operation.backup_staging.as_deref() {
                validate_exact_reserved_child(
                    path,
                    &state.paths.backups_dir,
                    &format!(
                        ".{}-staging-{}",
                        journal
                            .installation_id
                            .trim_start_matches("linux-")
                            .trim_start_matches("windows-"),
                        journal.transaction_id
                    ),
                )?;
            }
            if journal
                .after_record
                .as_ref()
                .and_then(|record| record.bundle_fingerprint.as_deref())
                .is_some_and(|fingerprint| fingerprint != operation.new_fingerprint)
            {
                return Err(recovery_conflict(
                    "The install after-state fingerprint does not match its journal.",
                    journal.installation_id.clone(),
                ));
            }
            if let Some(record) = journal.before_record.as_ref() {
                if record.bundle_fingerprint.as_deref() != operation.old_fingerprint.as_deref() {
                    return Err(recovery_conflict(
                        "The install before-state fingerprint does not match its journal.",
                        journal.installation_id.clone(),
                    ));
                }
            }
            if let Some(record) = journal.after_record.as_ref() {
                if record.backup_bundle_fingerprint.as_deref()
                    != operation.old_fingerprint.as_deref()
                {
                    return Err(recovery_conflict(
                        "The install rollback fingerprint does not match its after-state record.",
                        journal.installation_id.clone(),
                    ));
                }
                if journal
                    .after_integration
                    .iter()
                    .any(|snapshot| snapshot.contents.is_none())
                {
                    return Err(recovery_conflict(
                        "A committed install journal has an incomplete desktop integration snapshot.",
                        journal.installation_id.clone(),
                    ));
                }
            }
        }
        JournalOperation::Restore(operation) => {
            validate_exact_reserved_child(
                &operation.target_staging,
                target_parent,
                &format!(".aseprite-restore-staging-{}", journal.transaction_id),
            )?;
            validate_exact_reserved_child(
                &operation.target_previous,
                target_parent,
                &format!(".aseprite-restore-current-{}", journal.transaction_id),
            )?;
            validate_backup_children(
                state,
                [
                    Some(&operation.backup),
                    Some(&operation.backup_staging),
                    Some(&operation.backup_previous),
                ],
            )?;
            let backup_parent = operation.backup.parent().ok_or_else(|| {
                recovery_conflict(
                    "The restore rollback path has no parent.",
                    operation.backup.display().to_string(),
                )
            })?;
            validate_exact_reserved_child(
                &operation.backup_staging,
                backup_parent,
                &format!(
                    ".aseprite-restore-backup-staging-{}",
                    journal.transaction_id
                ),
            )?;
            validate_exact_reserved_child(
                &operation.backup_previous,
                backup_parent,
                &format!(".aseprite-restore-previous-{}", journal.transaction_id),
            )?;
            let before = journal.before_record.as_ref().ok_or_else(|| {
                recovery_conflict(
                    "The restore journal is missing its before-state ownership record.",
                    journal.installation_id.clone(),
                )
            })?;
            if before.bundle_fingerprint.as_deref() != Some(operation.current_fingerprint.as_str())
                || before.backup_bundle_fingerprint.as_deref()
                    != Some(operation.rollback_fingerprint.as_str())
            {
                return Err(recovery_conflict(
                    "The restore fingerprints do not match the before-state record.",
                    journal.installation_id.clone(),
                ));
            }
            if let Some(after) = journal.after_record.as_ref() {
                if after.bundle_fingerprint.as_deref()
                    != Some(operation.restored_fingerprint.as_str())
                    || after.backup_bundle_fingerprint.as_deref()
                        != Some(operation.current_fingerprint.as_str())
                {
                    return Err(recovery_conflict(
                        "The restore fingerprints do not match the after-state record.",
                        journal.installation_id.clone(),
                    ));
                }
                if journal
                    .after_integration
                    .iter()
                    .any(|snapshot| snapshot.contents.is_none())
                {
                    return Err(recovery_conflict(
                        "A committed restore journal has an incomplete desktop integration snapshot.",
                        journal.installation_id.clone(),
                    ));
                }
            }
        }
        JournalOperation::Uninstall(operation) => {
            if !operation.after_integration_paths.is_empty() || journal.after_record.is_some() {
                return Err(recovery_conflict(
                    "The uninstall journal contains an unexpected after-state desktop integration.",
                    journal.installation_id.clone(),
                ));
            }
            if journal
                .after_integration
                .iter()
                .any(|snapshot| snapshot.contents.is_some())
            {
                return Err(recovery_conflict(
                    "An uninstall journal contains a non-empty after-integration snapshot.",
                    journal.installation_id.clone(),
                ));
            }
            validate_backup_children(state, [operation.backup.as_ref()])?;
            if journal
                .before_record
                .as_ref()
                .and_then(|record| record.bundle_fingerprint.as_deref())
                != Some(operation.target_fingerprint.as_str())
            {
                return Err(recovery_conflict(
                    "The uninstall fingerprint does not match its ownership record.",
                    journal.installation_id.clone(),
                ));
            }
            if operation.backup.is_some() != operation.backup_fingerprint.is_some()
                || journal
                    .before_record
                    .as_ref()
                    .and_then(|record| record.backup_bundle_fingerprint.as_deref())
                    != operation.backup_fingerprint.as_deref()
            {
                return Err(recovery_conflict(
                    "The uninstall rollback fingerprint does not match its ownership record.",
                    journal.installation_id.clone(),
                ));
            }
        }
    }
    let quarantine_sources = match &journal.operation {
        JournalOperation::Install(operation) => {
            let mut sources = vec![
                (operation.target.clone(), operation.new_fingerprint.clone()),
                (operation.staging.clone(), operation.new_fingerprint.clone()),
            ];
            if let Some(fingerprint) = operation.old_fingerprint.as_ref() {
                sources.push((operation.previous.clone(), fingerprint.clone()));
                if let Some(path) = operation.backup_staging.as_ref() {
                    sources.push((path.clone(), fingerprint.clone()));
                }
                if let Some(path) = operation.backup.as_ref() {
                    sources.push((path.clone(), fingerprint.clone()));
                }
            }
            if let Some(path) = operation.superseded_backup.as_ref() {
                let fingerprint = journal
                    .before_record
                    .as_ref()
                    .and_then(|record| record.backup_bundle_fingerprint.clone())
                    .ok_or_else(|| {
                        recovery_conflict(
                            "A superseded rollback quarantine has no ownership fingerprint.",
                            path.display().to_string(),
                        )
                    })?;
                sources.push((path.clone(), fingerprint));
            }
            sources
        }
        JournalOperation::Restore(operation) => vec![
            (
                operation.target.clone(),
                operation.restored_fingerprint.clone(),
            ),
            (
                operation.target_staging.clone(),
                operation.restored_fingerprint.clone(),
            ),
            (
                operation.target_previous.clone(),
                operation.current_fingerprint.clone(),
            ),
            (
                operation.backup.clone(),
                operation.current_fingerprint.clone(),
            ),
            (
                operation.backup_staging.clone(),
                operation.current_fingerprint.clone(),
            ),
            (
                operation.backup_previous.clone(),
                operation.rollback_fingerprint.clone(),
            ),
        ],
        JournalOperation::Uninstall(operation) => {
            let mut sources = vec![(
                operation.target.clone(),
                operation.target_fingerprint.clone(),
            )];
            if let (Some(path), Some(fingerprint)) = (
                operation.backup.as_ref(),
                operation.backup_fingerprint.as_ref(),
            ) {
                sources.push((path.clone(), fingerprint.clone()));
            }
            sources
        }
    };
    let expected_quarantines = expected_quarantines(&journal.transaction_id, quarantine_sources)?;
    if journal.quarantines != expected_quarantines {
        return Err(recovery_conflict(
            "Transaction quarantine reservations do not exactly match their operation and identifier.",
            journal.transaction_id.clone(),
        ));
    }
    for entry in &journal.quarantines {
        ensure_real_transaction_parent(entry.source.parent().unwrap())?;
        ensure_real_transaction_parent(entry.quarantine.parent().unwrap())?;
    }
    Ok(())
}

fn validate_exact_reserved_child(path: &Path, parent: &Path, name: &str) -> AppResult<()> {
    if path.parent() != Some(parent) || path.file_name() != Some(OsStr::new(name)) {
        return Err(recovery_conflict(
            "A transaction path does not match its reserved name.",
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn validate_backup_children<const N: usize>(
    state: &AppState,
    paths: [Option<&PathBuf>; N],
) -> AppResult<()> {
    let allowed = std::fs::canonicalize(&state.paths.backups_dir)?;
    for path in paths.into_iter().flatten() {
        let parent = path.parent().ok_or_else(|| {
            recovery_conflict(
                "A transaction backup path has no parent directory.",
                path.display().to_string(),
            )
        })?;
        if std::fs::canonicalize(parent)? != allowed {
            return Err(recovery_conflict(
                "A transaction backup path is outside the installer backup directory.",
                path.display().to_string(),
            ));
        }
    }
    Ok(())
}

fn ensure_real_transaction_parent(path: &Path) -> AppResult<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&metadata)
        || !metadata.is_dir()
    {
        return Err(recovery_conflict(
            "A transaction parent path is not a real directory.",
            path.display().to_string(),
        ));
    }
    Ok(())
}

pub fn clean_cache(state: &AppState) -> AppResult<u64> {
    let size = directory_size(&state.paths.cache_dir);
    for directory in [&state.paths.archives_dir, &state.paths.builds_dir] {
        match std::fs::symlink_metadata(directory) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(InstallerError::with_detail(
                    "cacheLink",
                    "The installer cache cannot be cleaned through a symbolic link.",
                    directory.display().to_string(),
                ));
            }
            Ok(_) => std::fs::remove_dir_all(directory)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        std::fs::create_dir_all(directory)?;
    }
    Ok(size)
}

fn validate_record_identity(record: &ManagedRecord, target: &Path) -> AppResult<()> {
    if native::installation_id(&target.to_string_lossy()) != record.id {
        return Err(InstallerError::new(
            "managedIdentity",
            "The stored installation identity does not match its destination.",
        ));
    }
    let marker_path = target.join(".aseprite-installer.json");
    let marker: OwnershipMarker =
        serde_json::from_slice(&std::fs::read(&marker_path)?).map_err(|error| {
            InstallerError::with_detail(
                "ownershipMarker",
                "The managed-installation ownership marker is invalid.",
                error.to_string(),
            )
        })?;
    if marker.installation_id != record.id {
        return Err(InstallerError::new(
            "ownershipMarker",
            "The installation is not owned by this managed record.",
        ));
    }
    Ok(())
}

fn ensure_record_matches_target(record: &ManagedRecord, target: &Path) -> AppResult<()> {
    validate_record_identity(record, target)?;
    ensure_record_fingerprint(
        target,
        record.bundle_fingerprint.as_deref(),
        "managed installation",
    )
}

fn ensure_record_fingerprint(path: &Path, expected: Option<&str>, label: &str) -> AppResult<()> {
    let expected = expected.ok_or_else(|| {
        InstallerError::new(
            "missingFingerprint",
            format!("The {label} predates portable ownership verification and cannot be changed destructively."),
        )
    })?;
    let actual = hex::encode(native::artifact_fingerprint(path)?);
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(InstallerError::new(
            "installationChanged",
            format!("The {label} changed outside Aseprite Installer. It will not be replaced or removed."),
        ))
    }
}

fn ensure_installed_fingerprint(path: &Path, expected: &[u8; 32]) -> AppResult<()> {
    if &native::artifact_fingerprint(path)? == expected {
        Ok(())
    } else {
        Err(InstallerError::new(
            "installValidation",
            "The committed installation fingerprint changed unexpectedly.",
        ))
    }
}

fn validate_complete_artifact(root: &Path, expected_version: &str) -> AppResult<String> {
    let version = native::validate_artifact(root, expected_version)?;
    let icu_data = root.join("icudtl.dat");
    if !icu_data.is_file() {
        return Err(InstallerError::with_detail(
            "artifactIncomplete",
            "The built Aseprite artifact is missing Skia's ICU data file.",
            icu_data.display().to_string(),
        ));
    }
    Ok(version)
}

fn write_ownership_marker(
    staging: &Path,
    installation_id: &str,
    release: &ReleaseInfo,
    transaction_nonce: &str,
) -> AppResult<()> {
    let marker = OwnershipMarker {
        schema_version: 1,
        installation_id: installation_id.into(),
        source_tag: release.tag.clone(),
        source_digest: release.digest.clone(),
        transaction_nonce: Some(transaction_nonce.into()),
    };
    let path = staging.join(".aseprite-installer.json");
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&serde_json::to_vec_pretty(&marker)?)?;
    file.sync_all()?;
    Ok(())
}

fn ensure_backup_ownership_marker(
    backup: &Path,
    installation_id: &str,
    source_tag: &str,
    source_digest: &str,
    transaction_nonce: &str,
) -> AppResult<bool> {
    let path = backup.join(".aseprite-installer.json");
    if path.is_file() {
        let marker: OwnershipMarker =
            serde_json::from_slice(&std::fs::read(&path)?).map_err(|error| {
                InstallerError::with_detail(
                    "ownershipMarker",
                    "The existing managed ownership marker is invalid.",
                    error.to_string(),
                )
            })?;
        if marker.installation_id != installation_id {
            return Err(InstallerError::new(
                "ownershipMarker",
                "The rollback copy belongs to another managed installation.",
            ));
        }
        return Ok(false);
    }
    let marker = OwnershipMarker {
        schema_version: 1,
        installation_id: installation_id.into(),
        source_tag: source_tag.into(),
        source_digest: source_digest.into(),
        transaction_nonce: Some(transaction_nonce.into()),
    };
    let temporary = backup.join(format!(".aseprite-installer-{}.tmp", Uuid::new_v4()));
    let mut cleanup = CleanupFile::new(temporary.clone());
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    cleanup.arm();
    file.write_all(&serde_json::to_vec_pretty(&marker)?)?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = std::fs::rename(&temporary, &path) {
        return Err(error.into());
    }
    cleanup.disarm();
    Ok(true)
}

fn validate_source_asset(asset: &VerifiedAsset<'_>) -> AppResult<()> {
    if asset.size == 0
        || !asset.name.starts_with("Aseprite-v1.3")
        || !asset.name.ends_with("-Source.zip")
        || asset.name.contains(['/', '\\'])
        || asset.sha256.len() != 64
        || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !asset
            .url
            .starts_with("https://github.com/aseprite/aseprite/releases/download/")
    {
        return Err(InstallerError::new(
            "sourceAsset",
            "The selected Aseprite source asset metadata is unsafe.",
        ));
    }
    Ok(())
}

async fn download_verified_asset(
    state: &AppState,
    asset: VerifiedAsset<'_>,
    cancelled: &AtomicBool,
    progress: &Channel<OperationProgress>,
    progress_range: (u8, u8),
    label: &str,
) -> AppResult<PathBuf> {
    if asset.name.contains(['/', '\\'])
        || asset.sha256.len() != 64
        || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(InstallerError::new(
            "assetManifest",
            "A pinned download manifest is invalid.",
        ));
    }
    std::fs::create_dir_all(&state.paths.archives_dir)?;
    let destination = state.paths.archives_dir.join(asset.name);
    if destination.exists() {
        send_stage(
            progress,
            OperationStage::Verifying,
            Some(progress_range.0),
            format!("Checking the cached {label}…"),
        );
        if file_matches(&destination, asset.size, asset.sha256)? {
            return Ok(destination);
        }
        std::fs::remove_file(&destination)?;
    }

    let available = fs2::available_space(&state.paths.archives_dir)?;
    if available < asset.size.saturating_add(SOURCE_SPACE_MARGIN_BYTES) {
        return Err(InstallerError::new(
            "downloadSpace",
            format!("There is not enough free space to download the {label}."),
        ));
    }
    send_stage(
        progress,
        OperationStage::Downloading,
        Some(progress_range.0),
        format!("Downloading the {label}…"),
    );
    let partial = destination.with_extension(format!("zip.{}.part", Uuid::new_v4()));
    let mut cleanup = CleanupFile::new(partial.clone());
    let client = state.http_client()?;
    let mut pending = Box::pin(client.get(asset.url).send());
    let response = loop {
        ensure_not_cancelled(cancelled)?;
        match tokio::time::timeout(Duration::from_millis(200), pending.as_mut()).await {
            Ok(response) => break response,
            Err(_) => continue,
        }
    }
    .map_err(|error| {
        InstallerError::with_detail(
            "assetDownload",
            format!("The {label} could not be downloaded."),
            error.to_string(),
        )
    })?
    .error_for_status()
    .map_err(|error| {
        InstallerError::with_detail(
            "assetDownload",
            format!("GitHub did not provide the {label}."),
            error.to_string(),
        )
    })?;
    if response
        .content_length()
        .is_some_and(|length| length != asset.size)
    {
        return Err(InstallerError::new(
            "assetSize",
            format!("GitHub reported an unexpected size for the {label}."),
        ));
    }
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await?;
    cleanup.arm();
    let mut downloaded = 0_u64;
    let mut last_data = Instant::now();
    loop {
        ensure_not_cancelled(cancelled)?;
        let next = match tokio::time::timeout(Duration::from_millis(250), stream.next()).await {
            Ok(next) => next,
            Err(_) if last_data.elapsed() < Duration::from_secs(30) => continue,
            Err(_) => {
                return Err(InstallerError::new(
                    "assetDownloadTimeout",
                    format!("The {label} download stopped responding for 30 seconds."),
                ));
            }
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|error| {
            InstallerError::with_detail(
                "assetDownload",
                format!("The {label} download was interrupted."),
                error.to_string(),
            )
        })?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > asset.size {
            return Err(InstallerError::new(
                "assetSize",
                format!("GitHub sent more bytes than declared for the {label}."),
            ));
        }
        file.write_all(&chunk).await?;
        last_data = Instant::now();
        let width = progress_range.1.saturating_sub(progress_range.0);
        let percent = progress_range
            .0
            .saturating_add(((downloaded as f64 / asset.size as f64) * f64::from(width)) as u8);
        send_stage(
            progress,
            OperationStage::Downloading,
            Some(percent.min(progress_range.1)),
            format!("Downloading the {label}…"),
        );
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    if !file_matches(&partial, asset.size, asset.sha256)? {
        return Err(InstallerError::new(
            "checksumMismatch",
            format!("The downloaded {label} did not match its pinned size and SHA-256 digest."),
        ));
    }
    match std::fs::hard_link(&partial, &destination) {
        Ok(()) => {}
        Err(_) if destination.exists() && file_matches(&destination, asset.size, asset.sha256)? => {
        }
        Err(error) => {
            return Err(InstallerError::with_detail(
                "assetCacheCommit",
                format!("The verified {label} could not be committed to the cache."),
                error.to_string(),
            ));
        }
    }
    std::fs::remove_file(&partial)?;
    cleanup.disarm();
    Ok(destination)
}

fn file_matches(path: &Path, expected_size: u64, expected_sha256: &str) -> AppResult<bool> {
    let mut file = open_regular_no_follow(path, "archiveCache")?;
    if file.metadata()?.len() != expected_size {
        return Ok(false);
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    let mut bytes_read = 0_u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(count as u64);
        if bytes_read > expected_size {
            return Ok(false);
        }
        hasher.update(&buffer[..count]);
    }
    Ok(bytes_read == expected_size
        && hex::encode(hasher.finalize()).eq_ignore_ascii_case(expected_sha256))
}

fn open_regular_no_follow(path: &Path, code: &str) -> AppResult<std::fs::File> {
    let path_metadata = std::fs::symlink_metadata(path).map_err(|error| {
        InstallerError::with_detail(
            code,
            "A verified archive path could not be inspected safely.",
            format!("{}: {error}", path.display()),
        )
    })?;
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || metadata_is_reparse_point(&path_metadata)
    {
        return Err(InstallerError::with_detail(
            code,
            "A verified archive must be a regular file, not a link or reparse point.",
            path.display().to_string(),
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(target_os = "windows")]
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() || metadata_is_reparse_point(&opened) {
        return Err(InstallerError::with_detail(
            code,
            "A verified archive changed type while it was being opened.",
            path.display().to_string(),
        ));
    }
    #[cfg(unix)]
    if opened.dev() != path_metadata.dev() || opened.ino() != path_metadata.ino() {
        return Err(InstallerError::with_detail(
            code,
            "A verified archive changed identity while it was being opened.",
            path.display().to_string(),
        ));
    }
    Ok(file)
}

#[cfg(unix)]
fn metadata_is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn metadata_is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn read_verified_archive_bytes(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    cancelled: &AtomicBool,
) -> AppResult<Vec<u8>> {
    if expected_size == 0 || expected_size > MAX_ARCHIVE_BYTES {
        return Err(InstallerError::new(
            "archiveSize",
            "The verified archive size is outside the installer safety limit.",
        ));
    }
    let capacity = usize::try_from(expected_size).map_err(|_| {
        InstallerError::new(
            "archiveSize",
            "The verified archive is too large for this process architecture.",
        )
    })?;
    let mut file = open_regular_no_follow(path, "archiveIdentity")?;
    if file.metadata()?.len() != expected_size {
        return Err(InstallerError::new(
            "archiveSize",
            "The cached archive size changed before extraction.",
        ));
    }
    let mut contents = Vec::with_capacity(capacity);
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        ensure_not_cancelled(cancelled)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if contents.len().saturating_add(count) > capacity {
            return Err(InstallerError::new(
                "archiveSize",
                "The cached archive grew while it was being verified.",
            ));
        }
        contents.extend_from_slice(&buffer[..count]);
    }
    if contents.len() != capacity
        || !hex::encode(Sha256::digest(&contents)).eq_ignore_ascii_case(expected_sha256)
    {
        return Err(InstallerError::new(
            "checksumMismatch",
            "The cached archive changed before extraction and no bytes were used.",
        ));
    }
    Ok(contents)
}

fn create_output_no_follow(path: &Path) -> AppResult<std::fs::File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(target_os = "windows")]
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata_is_reparse_point(&metadata) {
        return Err(InstallerError::with_detail(
            "zipOutputType",
            "An extracted archive entry did not create a regular file.",
            path.display().to_string(),
        ));
    }
    Ok(file)
}

fn ensure_safe_extraction_directory(root: &Path, directory: &Path) -> AppResult<()> {
    let root_metadata = std::fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink()
        || !root_metadata.is_dir()
        || metadata_is_reparse_point(&root_metadata)
    {
        return Err(InstallerError::with_detail(
            "zipOutputType",
            "The reserved extraction workspace is not a real directory.",
            root.display().to_string(),
        ));
    }
    let root_canonical = std::fs::canonicalize(root)?;
    let relative = directory.strip_prefix(root).map_err(|error| {
        InstallerError::with_detail(
            "zipOutputPath",
            "An extraction directory escaped the reserved workspace.",
            error.to_string(),
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        let metadata = std::fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || metadata_is_reparse_point(&metadata)
        {
            return Err(InstallerError::with_detail(
                "zipOutputType",
                "An extraction directory became a link, reparse point, or non-directory.",
                current.display().to_string(),
            ));
        }
    }
    let canonical = std::fs::canonicalize(directory)?;
    if !canonical.starts_with(&root_canonical) {
        return Err(InstallerError::with_detail(
            "zipOutputPath",
            "An extraction directory resolved outside the reserved workspace.",
            canonical.display().to_string(),
        ));
    }
    Ok(())
}

fn validate_extracted_tree(root: &Path) -> AppResult<()> {
    let canonical_root = std::fs::canonicalize(root)?;
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| {
            InstallerError::with_detail(
                "zipOutputTree",
                "The extracted archive tree could not be inspected completely.",
                error.to_string(),
            )
        })?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink()
            || metadata_is_reparse_point(&metadata)
            || (!metadata.is_dir() && !metadata.is_file())
        {
            return Err(InstallerError::with_detail(
                "zipOutputType",
                "The extracted archive contains a link, reparse point, or special file.",
                entry.path().display().to_string(),
            ));
        }
        let canonical = std::fs::canonicalize(entry.path())?;
        if !canonical.starts_with(&canonical_root) {
            return Err(InstallerError::with_detail(
                "zipOutputPath",
                "An extracted archive entry resolved outside the reserved workspace.",
                canonical.display().to_string(),
            ));
        }
    }
    Ok(())
}

async fn extract_in_background(
    archive: PathBuf,
    destination: PathBuf,
    expected_size: u64,
    expected_sha256: String,
    cancelled: Arc<AtomicBool>,
) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        extract_archive_safely(
            &archive,
            &destination,
            expected_size,
            &expected_sha256,
            &cancelled,
        )
    })
    .await
    .map_err(|error| {
        InstallerError::with_detail(
            "extract",
            "A verified archive extraction task failed.",
            error.to_string(),
        )
    })?
}

fn extract_archive_safely(
    archive_path: &Path,
    destination: &Path,
    expected_size: u64,
    expected_sha256: &str,
    cancelled: &AtomicBool,
) -> AppResult<()> {
    std::fs::create_dir_all(destination)?;
    ensure_safe_extraction_directory(destination, destination)?;
    let archive_bytes =
        read_verified_archive_bytes(archive_path, expected_size, expected_sha256, cancelled)?;
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(archive_bytes)).map_err(|error| {
            InstallerError::with_detail(
                "zip",
                "A verified archive is not a valid ZIP file.",
                error.to_string(),
            )
        })?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(InstallerError::new(
            "zipEntries",
            "The archive contains too many entries to extract safely.",
        ));
    }
    let mut declared_bytes = 0_u64;
    let mut extracted_bytes = 0_u64;
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        ensure_not_cancelled(cancelled)?;
        let mut entry = archive.by_index(index).map_err(|error| {
            InstallerError::with_detail(
                "zip",
                "An archive entry could not be read.",
                error.to_string(),
            )
        })?;
        let enclosed = entry.enclosed_name().ok_or_else(|| {
            InstallerError::new(
                "zipSlip",
                "The archive contains a path outside its destination.",
            )
        })?;
        validate_archive_path(&enclosed)?;
        let normalized = enclosed.to_string_lossy().replace('\\', "/");
        #[cfg(target_os = "windows")]
        let normalized = normalized.to_ascii_lowercase();
        if !names.insert(normalized) {
            return Err(InstallerError::new(
                "zipCollision",
                "The archive contains duplicate or case-colliding paths.",
            ));
        }
        let mode = entry.unix_mode().unwrap_or(0);
        let file_type = mode & 0o170000;
        if file_type == 0o120000
            || (file_type != 0 && file_type != 0o040000 && file_type != 0o100000)
        {
            return Err(InstallerError::new(
                "zipSpecialFile",
                "The archive contains a link or special file.",
            ));
        }
        declared_bytes = declared_bytes.saturating_add(entry.size());
        if declared_bytes > MAX_EXTRACTED_BYTES {
            return Err(InstallerError::new(
                "zipSize",
                "The archive declares an unsafe expanded size.",
            ));
        }
        let output = destination.join(enclosed);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)?;
            ensure_safe_extraction_directory(destination, &output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
            ensure_safe_extraction_directory(destination, parent)?;
        }
        let mut output_file = create_output_no_follow(&output)?;
        let mut buffer = [0_u8; 128 * 1024];
        let mut entry_bytes = 0_u64;
        loop {
            ensure_not_cancelled(cancelled)?;
            let count = entry.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output_file.write_all(&buffer[..count])?;
            entry_bytes = entry_bytes.saturating_add(count as u64);
            extracted_bytes = extracted_bytes.saturating_add(count as u64);
            if entry_bytes > entry.size() || extracted_bytes > MAX_EXTRACTED_BYTES {
                return Err(InstallerError::new(
                    "zipSize",
                    "The archive expanded beyond its declared safe size.",
                ));
            }
        }
        if entry_bytes != entry.size() {
            return Err(InstallerError::new(
                "zipSize",
                "An archive entry did not expand to its declared size.",
            ));
        }
        #[cfg(unix)]
        if mode != 0 {
            use std::os::unix::fs::PermissionsExt;
            output_file.set_permissions(std::fs::Permissions::from_mode(mode & 0o777))?;
        }
        output_file.sync_all()?;
        ensure_safe_extraction_directory(destination, output.parent().unwrap_or(destination))?;
    }
    validate_extracted_tree(destination)?;
    Ok(())
}

fn validate_archive_path(path: &Path) -> AppResult<()> {
    for component in path.components() {
        let value = component.as_os_str().to_string_lossy();
        if value.contains('\0') || value.contains(':') {
            return Err(InstallerError::new(
                "zipPath",
                "The archive contains a platform-unsafe path.",
            ));
        }
        #[cfg(target_os = "windows")]
        {
            let stem = value
                .trim_end_matches([' ', '.'])
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            if matches!(
                stem.as_str(),
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "COM1"
                    | "COM2"
                    | "COM3"
                    | "COM4"
                    | "COM5"
                    | "COM6"
                    | "COM7"
                    | "COM8"
                    | "COM9"
                    | "LPT1"
                    | "LPT2"
                    | "LPT3"
                    | "LPT4"
                    | "LPT5"
                    | "LPT6"
                    | "LPT7"
                    | "LPT8"
                    | "LPT9"
            ) {
                return Err(InstallerError::new(
                    "zipPath",
                    "The archive contains a Windows reserved path.",
                ));
            }
        }
    }
    Ok(())
}

fn find_source_root(extracted: &Path) -> AppResult<PathBuf> {
    let mut candidates = Vec::new();
    for entry in WalkDir::new(extracted).max_depth(3).follow_links(false) {
        let entry = entry.map_err(|error| {
            InstallerError::with_detail(
                "sourceTree",
                "The extracted source tree could not be inspected.",
                error.to_string(),
            )
        })?;
        if entry.file_type().is_file() && entry.file_name() == OsStr::new("CMakeLists.txt") {
            let root = entry.path().parent().unwrap_or(extracted);
            if root.join("laf/misc/skia-tag.txt").is_file() {
                candidates.push(root.to_path_buf());
            }
        }
    }
    if candidates.len() == 1 {
        Ok(candidates.remove(0))
    } else {
        Err(InstallerError::new(
            "sourceRoot",
            "The verified archive does not contain exactly one recognizable Aseprite source root.",
        ))
    }
}

fn validate_declared_skia_tag(source_root: &Path) -> AppResult<()> {
    let value = std::fs::read_to_string(source_root.join("laf/misc/skia-tag.txt"))?;
    if value.trim() == SKIA_TAG {
        Ok(())
    } else {
        Err(InstallerError::with_detail(
            "unsupportedSkia",
            "The selected source requires a Skia build that this installer has not pinned and verified.",
            format!("Declared tag: {}", value.trim()),
        ))
    }
}

fn find_skia_root(extracted: &Path) -> AppResult<PathBuf> {
    if extracted.join("out/Release-x64/args.gn").is_file() {
        return Ok(extracted.to_path_buf());
    }
    let entries = std::fs::read_dir(extracted)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect::<Vec<_>>();
    if entries.len() == 1 && entries[0].path().join("out/Release-x64/args.gn").is_file() {
        Ok(entries[0].path())
    } else {
        Err(InstallerError::new(
            "skiaRoot",
            "The pinned Skia archive has an unexpected directory structure.",
        ))
    }
}

fn validate_skia_tree(root: &Path) -> AppResult<()> {
    let release = root.join("out/Release-x64");
    #[cfg(target_os = "linux")]
    let libraries = [
        "libskia.a",
        "libskunicode.a",
        "libskshaper.a",
        "libpng.a",
        "libwebp.a",
        "libfreetype2.a",
        "libharfbuzz.a",
    ];
    #[cfg(target_os = "windows")]
    let libraries = [
        "skia.lib",
        "skunicode.lib",
        "skshaper.lib",
        "libpng.lib",
        "libwebp.lib",
        "freetype2.lib",
        "harfbuzz.lib",
    ];
    for required in [
        root.join("include"),
        release.join("args.gn"),
        root.join("third_party/externals/icu/flutter/icudtl.dat"),
    ] {
        if !required.exists() {
            return Err(InstallerError::with_detail(
                "skiaIncomplete",
                "The pinned Skia archive is incomplete.",
                format!("Missing {}", required.display()),
            ));
        }
    }
    for library in libraries {
        let required = release.join(library);
        if !required.is_file() {
            return Err(InstallerError::with_detail(
                "skiaIncomplete",
                "The pinned Skia archive is missing a required static library.",
                required.display().to_string(),
            ));
        }
    }
    Ok(())
}

async fn run_streaming_command(
    mut command: Command,
    cancelled: &AtomicBool,
    progress: &Channel<OperationProgress>,
    log_file: &mut std::fs::File,
    log_path: &Path,
    label: &str,
) -> AppResult<()> {
    let process_tree = StreamingProcessTree::new()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(target_os = "windows")]
    process_tree.prepare(&mut command);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| {
        InstallerError::with_detail(
            "buildStart",
            format!("{label} could not start."),
            error.to_string(),
        )
    })?;
    if let Err(error) = process_tree.assign(&child) {
        let _ = process_tree.terminate();
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(error);
    }
    let process_id = child.id();
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_process_tree(&mut child, process_id, &process_tree).await;
            return Err(InstallerError::new(
                "buildOutput",
                "Build stdout is unavailable.",
            ));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            terminate_process_tree(&mut child, process_id, &process_tree).await;
            return Err(InstallerError::new(
                "buildOutput",
                "Build stderr is unavailable.",
            ));
        }
    };
    let (sender, mut receiver) = mpsc::channel(BUILD_OUTPUT_CHANNEL_CAPACITY);
    spawn_line_reader(stdout, sender.clone());
    spawn_line_reader(stderr, sender);
    let started = Instant::now();
    let monitor_result = 'monitor: loop {
        while let Ok(event) = receiver.try_recv() {
            if let Err(error) = handle_build_output(log_file, progress, event) {
                break 'monitor Err(error);
            }
        }
        if cancelled.load(Ordering::SeqCst) {
            break Err(InstallerError::new(
                "cancelled",
                format!("{label} was cancelled."),
            ));
        }
        if started.elapsed() > BUILD_TIMEOUT {
            break Err(InstallerError::with_detail(
                "buildTimeout",
                format!("{label} exceeded the three-hour safety timeout and was stopped."),
                format!("Technical log: {}", log_path.display()),
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => {
                break Err(InstallerError::with_detail(
                    "buildWait",
                    format!("{label} could not be monitored safely."),
                    error.to_string(),
                ));
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let status = match monitor_result {
        Ok(status) => status,
        Err(error) => {
            terminate_process_tree(&mut child, process_id, &process_tree).await;
            return Err(error);
        }
    };

    #[cfg(target_os = "windows")]
    if let Err(error) = process_tree.terminate() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(error);
    }
    #[cfg(unix)]
    if let Some(process_id) = process_id {
        // The direct compiler may exit while a descendant still owns the
        // inherited output pipes. Kill its isolated PGID before draining so a
        // nominally successful child cannot strand readers indefinitely.
        let group = format!("-{process_id}");
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &group])
            .status()
            .await;
    }

    loop {
        match tokio::time::timeout(BUILD_OUTPUT_DRAIN_TIMEOUT, receiver.recv()).await {
            Ok(Some(event)) => {
                if let Err(error) = handle_build_output(log_file, progress, event) {
                    terminate_process_tree(&mut child, process_id, &process_tree).await;
                    return Err(error);
                }
            }
            Ok(None) => break,
            Err(_) => {
                terminate_process_tree(&mut child, process_id, &process_tree).await;
                return Err(InstallerError::with_detail(
                    "buildOutputTimeout",
                    format!("{label} left a background process holding its output stream open."),
                    format!("Technical log: {}", log_path.display()),
                ));
            }
        }
    }
    if let Err(error) = log_file.sync_all() {
        terminate_process_tree(&mut child, process_id, &process_tree).await;
        return Err(error.into());
    }
    if status.success() {
        Ok(())
    } else {
        Err(InstallerError::with_detail(
            "buildFailed",
            format!("{label} failed."),
            format!(
                "Exit status: {status}. Technical log: {}",
                log_path.display()
            ),
        ))
    }
}

enum BuildOutputEvent {
    Line(String),
    ReaderError(String),
}

fn spawn_line_reader<R>(mut reader: R, sender: mpsc::Sender<BuildOutputEvent>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        let mut read_buffer = [0_u8; 8 * 1024];
        let mut pending = Vec::new();
        let mut truncated = false;
        loop {
            let count = match reader.read(&mut read_buffer).await {
                Ok(0) => {
                    if !pending.is_empty() {
                        let line = decode_build_line(&pending, truncated);
                        let _ = sender.send(BuildOutputEvent::Line(line)).await;
                    }
                    break;
                }
                Ok(count) => count,
                Err(error) => {
                    let _ = sender
                        .send(BuildOutputEvent::ReaderError(error.to_string()))
                        .await;
                    break;
                }
            };
            let mut start = 0;
            for (index, byte) in read_buffer[..count].iter().enumerate() {
                if *byte != b'\n' {
                    continue;
                }
                append_bounded_line(&mut pending, &read_buffer[start..index], &mut truncated);
                let line = decode_build_line(&pending, truncated);
                if sender.send(BuildOutputEvent::Line(line)).await.is_err() {
                    return;
                }
                pending.clear();
                truncated = false;
                start = index + 1;
            }
            append_bounded_line(&mut pending, &read_buffer[start..count], &mut truncated);
        }
    });
}

fn append_bounded_line(pending: &mut Vec<u8>, bytes: &[u8], truncated: &mut bool) {
    if bytes.is_empty() {
        return;
    }
    if bytes.len() >= BUILD_LOG_LINE_LIMIT_BYTES {
        pending.clear();
        pending.extend_from_slice(&bytes[bytes.len() - BUILD_LOG_LINE_LIMIT_BYTES..]);
        *truncated = true;
        return;
    }
    let overflow = pending
        .len()
        .saturating_add(bytes.len())
        .saturating_sub(BUILD_LOG_LINE_LIMIT_BYTES);
    if overflow > 0 {
        pending.drain(..overflow.min(pending.len()));
        *truncated = true;
    }
    pending.extend_from_slice(bytes);
}

fn decode_build_line(bytes: &[u8], truncated: bool) -> String {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    let decoded = String::from_utf8_lossy(bytes);
    if truncated {
        format!("[line truncated] {decoded}")
    } else {
        decoded.into_owned()
    }
}

fn handle_build_output(
    log_file: &mut std::fs::File,
    progress: &Channel<OperationProgress>,
    event: BuildOutputEvent,
) -> AppResult<()> {
    match event {
        BuildOutputEvent::Line(line) => write_log_line(log_file, progress, line),
        BuildOutputEvent::ReaderError(detail) => Err(InstallerError::with_detail(
            "buildOutput",
            "A compiler output stream could not be read completely.",
            detail,
        )),
    }
}

fn write_log_line(
    log_file: &mut std::fs::File,
    progress: &Channel<OperationProgress>,
    line: String,
) -> AppResult<()> {
    let line = tail_text(&line, 4_000);
    writeln!(log_file, "{line}")?;
    let _ = progress.send(OperationProgress::log(OperationStage::Compiling, line));
    Ok(())
}

struct StreamingProcessTree {
    #[cfg(target_os = "windows")]
    job: native::ProcessTreeJob,
}

impl StreamingProcessTree {
    fn new() -> AppResult<Self> {
        Ok(Self {
            #[cfg(target_os = "windows")]
            job: native::ProcessTreeJob::new()?,
        })
    }

    #[cfg(target_os = "windows")]
    fn prepare(&self, command: &mut tokio::process::Command) {
        let _ = self;
        native::ProcessTreeJob::prepare_tokio_command(command);
    }

    fn assign(&self, child: &tokio::process::Child) -> AppResult<()> {
        #[cfg(target_os = "windows")]
        self.job.assign_and_resume_tokio_child(child)?;
        #[cfg(not(target_os = "windows"))]
        let _ = child;
        Ok(())
    }

    fn terminate(&self) -> AppResult<()> {
        #[cfg(target_os = "windows")]
        self.job.terminate()?;
        Ok(())
    }
}

async fn terminate_process_tree(
    child: &mut tokio::process::Child,
    process_id: Option<u32>,
    process_tree: &StreamingProcessTree,
) {
    #[cfg(unix)]
    if let Some(process_id) = process_id {
        let group = format!("-{process_id}");
        let _ = Command::new("/bin/kill")
            .args(["-TERM", "--", &group])
            .status()
            .await;
        for _ in 0..20 {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &group])
            .status()
            .await;
    }
    #[cfg(target_os = "windows")]
    let _ = process_id;
    let _ = process_tree.terminate();
    let _ = child.kill().await;
    let _ = child.wait().await;
}

fn ensure_copy_capacity(
    source: &Path,
    destination_directory: &Path,
    code: &str,
    message: &str,
) -> AppResult<()> {
    let required = checked_tree_size(source)?.saturating_add(SOURCE_SPACE_MARGIN_BYTES);
    let available = fs2::available_space(destination_directory).map_err(|error| {
        InstallerError::with_detail(
            code,
            "Free space on a transaction volume could not be inspected.",
            format!("{}: {error}", destination_directory.display()),
        )
    })?;
    if available < required {
        return Err(InstallerError::with_detail(
            code,
            message,
            format!(
                "{} bytes are available; {} bytes are required.",
                available, required
            ),
        ));
    }
    Ok(())
}

fn checked_tree_size(root: &Path) -> AppResult<u64> {
    let root_metadata = std::fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(InstallerError::with_detail(
            "artifactType",
            "A transaction source must be a real directory.",
            root.display().to_string(),
        ));
    }
    let mut bytes = 0_u64;
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| {
            InstallerError::with_detail(
                "artifactSize",
                "A transaction source could not be inspected completely.",
                error.to_string(),
            )
        })?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            return Err(InstallerError::with_detail(
                "artifactLink",
                "Transaction sources cannot contain symbolic links or junctions.",
                entry.path().display().to_string(),
            ));
        }
        if metadata.is_file() {
            bytes = bytes.checked_add(metadata.len()).ok_or_else(|| {
                InstallerError::new(
                    "artifactSize",
                    "The transaction source is too large to measure safely.",
                )
            })?;
        } else if !metadata.is_dir() {
            return Err(InstallerError::with_detail(
                "artifactType",
                "Transaction sources cannot contain special files.",
                entry.path().display().to_string(),
            ));
        }
    }
    Ok(bytes)
}

fn copy_tree(source: &Path, destination: &Path, cancelled: &AtomicBool) -> AppResult<()> {
    if destination.exists() {
        return Err(InstallerError::new(
            "stagingExists",
            "The reserved staging destination is already occupied.",
        ));
    }
    std::fs::create_dir(destination)?;
    let result = (|| {
        let mut directory_permissions = vec![(
            destination.to_path_buf(),
            std::fs::symlink_metadata(source)?.permissions(),
        )];
        for entry in WalkDir::new(source).follow_links(false) {
            ensure_not_cancelled(cancelled)?;
            let entry = entry.map_err(|error| {
                InstallerError::with_detail(
                    "artifactCopy",
                    "The built artifact could not be inspected.",
                    error.to_string(),
                )
            })?;
            if entry.path() == source {
                continue;
            }
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() || metadata_is_reparse_point(&metadata) {
                return Err(InstallerError::new(
                    "artifactLink",
                    "The built artifact contains an unsupported symbolic link.",
                ));
            }
            let relative = entry.path().strip_prefix(source).map_err(|error| {
                InstallerError::with_detail(
                    "artifactCopy",
                    "The built artifact contains an inconsistent path.",
                    error.to_string(),
                )
            })?;
            let output = destination.join(relative);
            if metadata.is_dir() {
                std::fs::create_dir(&output)?;
                directory_permissions.push((output, metadata.permissions()));
            } else if metadata.is_file() {
                let mut input = open_regular_no_follow(entry.path(), "artifactCopy")?;
                let mut output_file = create_output_no_follow(&output)?;
                std::io::copy(&mut input, &mut output_file)?;
                output_file.sync_all()?;
                drop(output_file);
                std::fs::set_permissions(&output, metadata.permissions())?;
            } else {
                return Err(InstallerError::new(
                    "artifactType",
                    "The built artifact contains an unsupported special file.",
                ));
            }
        }
        directory_permissions.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
        for (directory, permissions) in directory_permissions {
            std::fs::set_permissions(directory, permissions)?;
        }
        Ok::<(), InstallerError>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(destination);
    }
    result
}

fn ensure_supported_target(target: &Path, expected_to_exist: bool) -> AppResult<()> {
    if !target.is_absolute() || target.parent().is_none() {
        return Err(InstallerError::new(
            "targetPath",
            "The managed installation target must be an absolute directory path.",
        ));
    }
    match std::fs::symlink_metadata(target) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || metadata_is_reparse_point(&metadata)
                || !metadata.is_dir() =>
        {
            return Err(InstallerError::new(
                "targetType",
                "The managed destination is not a normal directory.",
            ));
        }
        Ok(_) if !expected_to_exist => {
            return Err(InstallerError::new(
                "targetOccupied",
                "The managed destination became occupied before installation.",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !expected_to_exist => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(InstallerError::new(
                "targetMissing",
                "The selected installation disappeared before it could be replaced.",
            ));
        }
        Err(error) => return Err(error.into()),
    }
    let parent = target.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    let parent_metadata = std::fs::symlink_metadata(parent)?;
    if parent_metadata.file_type().is_symlink()
        || metadata_is_reparse_point(&parent_metadata)
        || !parent_metadata.is_dir()
    {
        return Err(InstallerError::new(
            "targetParentLink",
            "The managed destination parent cannot be a symbolic link or junction.",
        ));
    }
    Ok(())
}

fn ensure_aseprite_is_closed(target: &Path) -> AppResult<()> {
    if native::target_aseprite_running(target)? {
        Err(InstallerError::new(
            "asepriteRunning",
            "Close Aseprite and check again before changing this installation.",
        ))
    } else {
        Ok(())
    }
}

fn unsupported_source_error() -> InstallerError {
    InstallerError::new(
        "unsupportedRelease",
        "The selected Aseprite source release does not have a supported native build plan.",
    )
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
    message: impl Into<String>,
) {
    let _ = progress.send(OperationProgress::stage(stage, percent, message));
}

fn build_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2)
        .clamp(1, 8)
}

fn tail_text(value: &str, maximum_characters: usize) -> String {
    let mut characters = value
        .chars()
        .rev()
        .take(maximum_characters)
        .collect::<Vec<_>>();
    characters.reverse();
    characters.into_iter().collect()
}

fn prune_files(directory: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| {
        std::cmp::Reverse(
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok(),
        )
    });
    for entry in entries.into_iter().skip(keep) {
        let _ = std::fs::remove_file(entry.path());
    }
}

fn directory_size(directory: &Path) -> u64 {
    WalkDir::new(directory)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok().map(|metadata| metadata.len()))
        .sum()
}

struct CleanupFile {
    path: PathBuf,
    armed: bool,
}

struct CleanupDirectory {
    path: PathBuf,
    armed: bool,
}

impl CleanupDirectory {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CleanupDirectory {
    fn drop(&mut self) {
        if self.armed {
            // Once the ownership marker exists, the directory may already be
            // the fingerprinted side of a durable transaction. Preserve it so
            // journal recovery can quarantine it with the exact transaction
            // reservation instead of recursively deleting it from Drop.
            match std::fs::symlink_metadata(self.path.join(".aseprite-installer.json")) {
                Ok(_) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return,
            }
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

impl CleanupFile {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: false }
    }

    fn arm(&mut self) {
        self.armed = true;
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CleanupFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_skia_manifest_is_complete() {
        assert!(SKIA_ASSET.url.contains(SKIA_TAG));
        assert!(SKIA_ASSET.name.ends_with("-x64.zip"));
        assert_eq!(SKIA_ASSET.sha256.len(), 64);
        const { assert!(SKIA_ASSET.size > 20_000_000) };
    }

    #[test]
    fn rejects_unsafe_source_asset_metadata() {
        let asset = VerifiedAsset {
            name: "../source.zip",
            url: "https://example.com/source.zip",
            size: 1,
            sha256: "not-a-digest",
        };
        assert_eq!(
            validate_source_asset(&asset).unwrap_err().code,
            "sourceAsset"
        );
    }

    #[test]
    fn ownership_marker_round_trips() {
        let marker = OwnershipMarker {
            schema_version: 1,
            installation_id: "linux-test".into(),
            source_tag: "v1.3.18.1".into(),
            source_digest: format!("sha256:{}", "a".repeat(64)),
            transaction_nonce: Some(Uuid::new_v4().to_string()),
        };
        let encoded = serde_json::to_vec(&marker).unwrap();
        let decoded: OwnershipMarker = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.installation_id, marker.installation_id);
    }

    #[test]
    fn adoption_only_applies_to_unmanaged_existing_targets() {
        assert!(manual_adoption_required(true, false));
        assert!(!manual_adoption_required(true, true));
        assert!(!manual_adoption_required(false, false));
    }

    #[test]
    fn desktop_integration_path_sets_ignore_order_but_not_profile_changes() {
        let first = vec![PathBuf::from("/profile/a"), PathBuf::from("/profile/b")];
        let reordered = vec![PathBuf::from("/profile/b"), PathBuf::from("/profile/a")];
        let changed = vec![PathBuf::from("/other/a"), PathBuf::from("/other/b")];
        assert!(integration_path_sets_equal(&first, &reordered));
        assert!(!integration_path_sets_equal(&first, &changed));
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn handle_bound_cleanup_removes_only_the_opened_tree() {
        let directory = tempfile::tempdir().unwrap();
        let quarantine = directory.path().join("quarantine");
        std::fs::create_dir_all(quarantine.join("nested/deeper")).unwrap();
        std::fs::write(quarantine.join("root.txt"), b"root").unwrap();
        std::fs::write(quarantine.join("nested/deeper/file.txt"), b"nested").unwrap();

        remove_real_tree_no_links(&quarantine).unwrap();

        assert!(!quarantine.exists());
        assert!(directory.path().exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn descriptor_relative_cleanup_refuses_links_and_preserves_their_target() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let quarantine = directory.path().join("quarantine");
        let outside = directory.path().join("outside");
        std::fs::create_dir(&quarantine).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), b"keep").unwrap();
        symlink(&outside, quarantine.join("escape")).unwrap();

        let error = remove_real_tree_no_links(&quarantine).unwrap_err();

        assert_eq!(error.code, "recoveryConflict");
        assert_eq!(std::fs::read(outside.join("keep.txt")).unwrap(), b"keep");
        assert!(quarantine.join("escape").exists());
    }
}
