use crate::error::{AppResult, InstallerError};
use crate::models::{ManagedRecord, ManagedState};
use crate::state::{replace_file_durable, CommitDurability, InstallerPaths};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::{Read, Write};
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;
#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
#[cfg(target_os = "windows")]
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
#[cfg(target_os = "windows")]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use walkdir::WalkDir;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FileRenameInfo, GetFileInformationByHandle, GetFinalPathNameByHandleW,
    SetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

pub(crate) const PORTABLE_JOURNAL_SCHEMA_VERSION: u32 = 2;
// Integration contents are serialized as JSON byte arrays. Two complete Linux
// sides can therefore expand well beyond their raw byte size while remaining
// within the per-file bound below.
const MAX_JOURNAL_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_INTEGRATION_FILE_BYTES: usize = 1024 * 1024;
#[cfg(target_os = "windows")]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
#[cfg(target_os = "windows")]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum JournalPhase {
    Prepared,
    TargetPreserved,
    TargetActivated,
    BackupActivated,
    IntegrationApplied,
    CommitReady,
    RegistryCommitted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PortableJournal {
    pub schema_version: u32,
    pub transaction_id: String,
    pub installation_id: String,
    /// Independent nonce bound into every quarantine proof. It is deliberately
    /// separate from the predictable reserved path names so a stale proof from
    /// another transaction cannot authorize recursive cleanup.
    pub quarantine_nonce: String,
    pub phase: JournalPhase,
    pub before_state_sha256: String,
    #[serde(default)]
    pub after_state_sha256: Option<String>,
    #[serde(default)]
    pub before_record: Option<ManagedRecord>,
    #[serde(default)]
    pub after_record: Option<ManagedRecord>,
    pub quarantines: Vec<QuarantineEntry>,
    pub before_integration: Vec<IntegrationSnapshot>,
    pub after_integration: Vec<IntegrationSnapshot>,
    pub operation: JournalOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuarantineEntry {
    pub source: PathBuf,
    pub quarantine: PathBuf,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntegrationSnapshot {
    pub path: PathBuf,
    #[serde(default)]
    pub contents: Option<Vec<u8>>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub unix_mode: Option<u32>,
}

impl IntegrationSnapshot {
    pub(crate) fn absent(path: PathBuf) -> Self {
        Self {
            path,
            contents: None,
            sha256: None,
            unix_mode: None,
        }
    }

    pub(crate) fn file(
        path: PathBuf,
        contents: Vec<u8>,
        unix_mode: Option<u32>,
    ) -> AppResult<Self> {
        if contents.len() > MAX_INTEGRATION_FILE_BYTES {
            return Err(InstallerError::with_detail(
                "desktopIntegrationSize",
                "A desktop integration file exceeds the transaction snapshot safety limit.",
                format!("{}: {} bytes", path.display(), contents.len()),
            ));
        }
        let sha256 = hex::encode(Sha256::digest(&contents));
        Ok(Self {
            path,
            contents: Some(contents),
            sha256: Some(sha256),
            unix_mode,
        })
    }

    pub(crate) fn validate(&self) -> AppResult<()> {
        match (&self.contents, &self.sha256) {
            (None, None) if self.unix_mode.is_none() => Ok(()),
            (Some(contents), Some(expected)) if contents.len() <= MAX_INTEGRATION_FILE_BYTES => {
                validate_hex_sha256(expected)?;
                let actual = hex::encode(Sha256::digest(contents));
                if &actual != expected {
                    return Err(InstallerError::with_detail(
                        "recoveryConflict",
                        "A desktop integration snapshot does not match its recorded digest.",
                        self.path.display().to_string(),
                    ));
                }
                if self
                    .unix_mode
                    .is_some_and(|mode| mode & !0o777 != 0 || mode & 0o022 != 0)
                {
                    return Err(InstallerError::with_detail(
                        "recoveryConflict",
                        "A desktop integration snapshot contains unsafe permissions.",
                        self.path.display().to_string(),
                    ));
                }
                Ok(())
            }
            _ => Err(InstallerError::with_detail(
                "recoveryConflict",
                "A desktop integration snapshot is incomplete or exceeds its safety limit.",
                self.path.display().to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "details", rename_all = "camelCase")]
pub(crate) enum JournalOperation {
    Install(InstallJournal),
    Restore(RestoreJournal),
    Uninstall(UninstallJournal),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstallJournal {
    pub target: PathBuf,
    pub staging: PathBuf,
    pub previous: PathBuf,
    pub backup_staging: Option<PathBuf>,
    pub backup: Option<PathBuf>,
    pub superseded_backup: Option<PathBuf>,
    pub old_fingerprint: Option<String>,
    pub new_fingerprint: String,
    pub before_integration_paths: Vec<PathBuf>,
    pub after_integration_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestoreJournal {
    pub target: PathBuf,
    pub target_staging: PathBuf,
    pub target_previous: PathBuf,
    pub backup: PathBuf,
    pub backup_staging: PathBuf,
    pub backup_previous: PathBuf,
    pub current_fingerprint: String,
    pub rollback_fingerprint: String,
    pub restored_fingerprint: String,
    pub before_integration_paths: Vec<PathBuf>,
    pub after_integration_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UninstallJournal {
    pub target: PathBuf,
    pub target_fingerprint: String,
    pub backup: Option<PathBuf>,
    pub backup_fingerprint: Option<String>,
    pub before_integration_paths: Vec<PathBuf>,
    pub after_integration_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryDirection {
    BeforeCommit,
    AfterCommit,
}

pub(crate) fn managed_state_sha256(state: &ManagedState) -> AppResult<String> {
    let encoded = serde_json::to_vec(state)?;
    let mut hasher = Sha256::new();
    hasher.update(b"aseprite-installer/managed-state/v1\0");
    hasher.update((encoded.len() as u64).to_le_bytes());
    hasher.update(encoded);
    Ok(hex::encode(hasher.finalize()))
}

pub(crate) fn recovery_direction(
    journal: &PortableJournal,
    current_state_sha256: &str,
) -> AppResult<RecoveryDirection> {
    if current_state_sha256 == journal.before_state_sha256 {
        return Ok(RecoveryDirection::BeforeCommit);
    }
    if journal.after_state_sha256.as_deref() == Some(current_state_sha256) {
        return Ok(RecoveryDirection::AfterCommit);
    }
    Err(InstallerError::with_detail(
        "recoveryConflict",
        "The managed-installation registry no longer matches either side of the interrupted transaction.",
        format!(
            "Transaction {}; current registry fingerprint {current_state_sha256}",
            journal.transaction_id
        ),
    ))
}

pub(crate) fn load_journal(paths: &InstallerPaths) -> AppResult<Option<PortableJournal>> {
    let path = &paths.transaction_file;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    #[cfg(target_os = "windows")]
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(recovery_io(
                "The transaction journal could not be opened safely.",
                path,
                error,
            ))
        }
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata_is_reparse(&metadata) {
        return Err(InstallerError::with_detail(
            "recoveryConflict",
            "The transaction journal is not a regular non-reparse file.",
            path.display().to_string(),
        ));
    }
    if metadata.len() > MAX_JOURNAL_BYTES {
        return Err(InstallerError::with_detail(
            "recoveryConflict",
            "The transaction journal exceeds its safety limit.",
            format!("{} bytes", metadata.len()),
        ));
    }
    let mut encoded = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut encoded)?;
    if encoded.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(InstallerError::new(
            "recoveryConflict",
            "The transaction journal grew while it was being read.",
        ));
    }
    let journal: PortableJournal = serde_json::from_slice(&encoded).map_err(|error| {
        InstallerError::with_detail(
            "recoveryConflict",
            "The transaction journal is invalid.",
            error.to_string(),
        )
    })?;
    validate_journal_header(&journal)?;
    Ok(Some(journal))
}

pub(crate) fn write_journal(paths: &InstallerPaths, journal: &PortableJournal) -> AppResult<()> {
    validate_journal_header(journal)?;
    let encoded = serde_json::to_vec_pretty(journal)?;
    if encoded.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(InstallerError::new(
            "transactionJournal",
            "The transaction journal exceeds its safety limit.",
        ));
    }
    let path = &paths.transaction_file;
    let parent = path.parent().ok_or_else(|| {
        InstallerError::new("transactionJournal", "The journal path has no parent.")
    })?;
    std::fs::create_dir_all(parent)?;
    validate_existing_journal(path)?;
    let temporary = parent.join(format!(".portable-transaction-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        #[cfg(target_os = "windows")]
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        let mut file = options.open(&temporary)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata_is_reparse(&metadata) {
            return Err(InstallerError::new(
                "transactionJournal",
                "The temporary transaction journal is not a regular file.",
            ));
        }
        file.write_all(&encoded)?;
        file.sync_all()?;
        drop(file);
        match replace_file_durable(&temporary, path)? {
            CommitDurability::Durable => {}
            CommitDurability::Uncertain(detail) => {
                return Err(InstallerError::with_detail(
                    "transactionJournalDurability",
                    "The transaction journal was replaced but could not be proven durable.",
                    detail,
                ));
            }
        }
        sync_directory(parent)?;
        Ok::<(), InstallerError>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn remove_journal(paths: &InstallerPaths) -> AppResult<()> {
    let path = &paths.transaction_file;
    validate_existing_journal(path)?;
    match std::fs::remove_file(path) {
        Ok(()) => {
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(recovery_io(
            "The completed transaction journal could not be removed.",
            path,
            error,
        )),
    }
}

pub(crate) fn durable_rename_no_replace(source: &Path, destination: &Path) -> AppResult<()> {
    durable_rename_no_replace_kind(source, destination, true)
}

pub(crate) fn durable_rename_file_no_replace(source: &Path, destination: &Path) -> AppResult<()> {
    durable_rename_no_replace_kind(source, destination, false)
}

fn durable_rename_no_replace_kind(
    source: &Path,
    destination: &Path,
    directory: bool,
) -> AppResult<()> {
    let source_parent = source.parent().ok_or_else(|| {
        InstallerError::new("transactionRename", "The source path has no parent.")
    })?;
    let destination_parent = destination.parent().ok_or_else(|| {
        InstallerError::new("transactionRename", "The destination path has no parent.")
    })?;
    if source_parent != destination_parent {
        return Err(InstallerError::with_detail(
            "transactionVolume",
            "Transaction activation paths must share the same parent directory.",
            format!("{} -> {}", source.display(), destination.display()),
        ));
    }
    validate_absolute_normal_path(source)?;
    validate_absolute_normal_path(destination)?;
    validate_directory_chain(source_parent).map_err(|error| {
        installer_error_context("validating the transaction parent before inspection", error)
    })?;
    match std::fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(InstallerError::with_detail(
                "transactionCollision",
                "A no-replace transaction destination is already occupied.",
                destination.display().to_string(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(installer_error_context(
                "checking whether the transaction destination is occupied",
                error.into(),
            ))
        }
    }
    let metadata = std::fs::symlink_metadata(source).map_err(|error| {
        installer_error_context("inspecting the transaction source", error.into())
    })?;
    if metadata.file_type().is_symlink()
        || metadata_is_reparse(&metadata)
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        return Err(InstallerError::with_detail(
            "transactionType",
            "Only a real transaction entry of the expected type can be activated.",
            source.display().to_string(),
        ));
    }
    // Revalidate immediately before the atomic syscall. Linux binds renameat2
    // to an O_NOFOLLOW directory fd; Windows binds the source and its in-place
    // destination name to a verified, non-delete-shared source handle below.
    validate_directory_chain(source_parent).map_err(|error| {
        installer_error_context("revalidating the transaction parent before rename", error)
    })?;
    platform_rename_no_replace(source, destination, directory).map_err(|error| {
        installer_error_context("performing the atomic no-replace rename", error.into())
    })?;
    validate_directory_chain(source_parent).map_err(|error| {
        installer_error_context("revalidating the transaction parent after rename", error)
    })?;
    let activated = std::fs::symlink_metadata(destination).map_err(|error| {
        installer_error_context("inspecting the activated transaction entry", error.into())
    })?;
    if activated.file_type().is_symlink()
        || metadata_is_reparse(&activated)
        || if directory {
            !activated.is_dir()
        } else {
            !activated.is_file()
        }
    {
        return Err(InstallerError::with_detail(
            "transactionType",
            "The activated transaction entry changed type across its rename.",
            destination.display().to_string(),
        ));
    }
    sync_directory(source_parent)
        .map_err(|error| installer_error_context("syncing the transaction parent", error))?;
    Ok(())
}

fn installer_error_context(stage: &str, mut error: InstallerError) -> InstallerError {
    let detail = error.detail.take().unwrap_or_else(|| error.message.clone());
    error.detail = Some(format!("{stage} failed: {detail}"));
    error
}

fn validate_absolute_normal_path(path: &Path) -> AppResult<()> {
    if !path.is_absolute()
        || path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Prefix(_)
                    | std::path::Component::RootDir
                    | std::path::Component::Normal(_)
            )
        })
    {
        return Err(InstallerError::with_detail(
            "transactionPath",
            "A transaction path must be absolute and contain no traversal components.",
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn validate_directory_chain(path: &Path) -> AppResult<()> {
    validate_absolute_normal_path(path)?;
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if !current.is_absolute() {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink() || metadata_is_reparse(&metadata) || !metadata.is_dir()
        {
            return Err(InstallerError::with_detail(
                "transactionParentLink",
                "A transaction directory chain contains a link, junction, or non-directory.",
                current.display().to_string(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn sync_tree_durable(root: &Path) -> AppResult<()> {
    let mut directories = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| {
            InstallerError::with_detail(
                "transactionSync",
                "A staged transaction tree could not be enumerated.",
                error.to_string(),
            )
        })?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || metadata_is_reparse(&metadata) {
            return Err(InstallerError::with_detail(
                "transactionSync",
                "A staged transaction tree contains a link or reparse point.",
                entry.path().display().to_string(),
            ));
        }
        if metadata.is_file() {
            #[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
            File::open(entry.path())?.sync_all()?;
            #[cfg(target_os = "windows")]
            {
                let mut options = OpenOptions::new();
                options
                    .read(true)
                    .write(true)
                    .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
                options.open(entry.path())?.sync_all()?;
            }
        } else if metadata.is_dir() {
            directories.push(entry.path().to_path_buf());
        } else {
            return Err(InstallerError::with_detail(
                "transactionSync",
                "A staged transaction tree contains a special file.",
                entry.path().display().to_string(),
            ));
        }
    }
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        sync_directory(&directory)?;
    }
    if let Some(parent) = root.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn validate_journal_header(journal: &PortableJournal) -> AppResult<()> {
    if journal.schema_version != PORTABLE_JOURNAL_SCHEMA_VERSION {
        return Err(InstallerError::with_detail(
            "recoveryConflict",
            "The transaction journal uses an unsupported schema version.",
            journal.schema_version.to_string(),
        ));
    }
    Uuid::parse_str(&journal.transaction_id).map_err(|error| {
        InstallerError::with_detail(
            "recoveryConflict",
            "The transaction journal identifier is invalid.",
            error.to_string(),
        )
    })?;
    Uuid::parse_str(&journal.quarantine_nonce).map_err(|error| {
        InstallerError::with_detail(
            "recoveryConflict",
            "The transaction quarantine nonce is invalid.",
            error.to_string(),
        )
    })?;
    if journal.installation_id.is_empty()
        || journal.installation_id.len() > 128
        || !journal
            .installation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(InstallerError::new(
            "recoveryConflict",
            "The transaction installation identifier is invalid.",
        ));
    }
    for value in [
        Some(journal.before_state_sha256.as_str()),
        journal.after_state_sha256.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_hex_sha256(value)?;
    }
    if journal.quarantines.len() > 16 {
        return Err(InstallerError::new(
            "recoveryConflict",
            "The transaction journal contains too many quarantine entries.",
        ));
    }
    for entry in &journal.quarantines {
        validate_hex_sha256(&entry.fingerprint)?;
        if !entry.source.is_absolute() || !entry.quarantine.is_absolute() {
            return Err(InstallerError::with_detail(
                "recoveryConflict",
                "A transaction quarantine path is not absolute.",
                entry.quarantine.display().to_string(),
            ));
        }
    }
    for snapshot in journal
        .before_integration
        .iter()
        .chain(&journal.after_integration)
    {
        snapshot.validate()?;
    }
    match &journal.operation {
        JournalOperation::Install(operation) => {
            validate_hex_sha256(&operation.new_fingerprint)?;
            if let Some(value) = operation.old_fingerprint.as_deref() {
                validate_hex_sha256(value)?;
            }
        }
        JournalOperation::Restore(operation) => {
            validate_hex_sha256(&operation.current_fingerprint)?;
            validate_hex_sha256(&operation.rollback_fingerprint)?;
            validate_hex_sha256(&operation.restored_fingerprint)?;
        }
        JournalOperation::Uninstall(operation) => {
            validate_hex_sha256(&operation.target_fingerprint)?;
            if let Some(value) = operation.backup_fingerprint.as_deref() {
                validate_hex_sha256(value)?;
            }
        }
    }
    Ok(())
}

fn validate_hex_sha256(value: &str) -> AppResult<()> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(InstallerError::with_detail(
            "recoveryConflict",
            "The transaction journal contains an invalid SHA-256 fingerprint.",
            value,
        ))
    }
}

fn validate_existing_journal(path: &Path) -> AppResult<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata_is_reparse(&metadata) => {
            Err(InstallerError::with_detail(
                "recoveryConflict",
                "The transaction journal path is not a regular non-reparse file.",
                path.display().to_string(),
            ))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(target_os = "linux")]
fn platform_rename_no_replace(
    source: &Path,
    destination: &Path,
    _directory: bool,
) -> std::io::Result<()> {
    let parent = source.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "the source has no parent")
    })?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let parent = options.open(parent)?;
    let source = std::ffi::CString::new(
        source
            .file_name()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "the source has no name")
            })?
            .as_bytes(),
    )
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the transaction source contains a NUL byte",
        )
    })?;
    let destination = std::ffi::CString::new(
        destination
            .file_name()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the destination has no name",
                )
            })?
            .as_bytes(),
    )
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the transaction destination contains a NUL byte",
        )
    })?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(test, target_os = "macos"))]
fn platform_rename_no_replace(
    source: &Path,
    destination: &Path,
    _directory: bool,
) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(target_os = "windows")]
fn platform_rename_no_replace(
    source: &Path,
    destination: &Path,
    directory: bool,
) -> std::io::Result<()> {
    let parent = source.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the transaction source has no parent",
        )
    })?;
    let destination_name = destination.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the transaction destination has no file name",
        )
    })?;

    // Normalized final handle paths reveal any junction followed while opening
    // an ancestor. Requiring the parent handle to resolve to the exact lexical
    // parent closes the validation-to-syscall junction-swap window.
    let expected_parent = normalize_windows_handle_path(parent.as_os_str());
    // Denying delete sharing keeps the verified parent stable through source
    // validation and the syscall. Child entries can still be renamed because
    // the directory handle shares reads and writes.
    let parent_handle =
        open_windows_rename_handle(parent, FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
            .map_err(|error| windows_io_context("opening the transaction parent", error))?;
    validate_windows_handle_type(&parent_handle, true)
        .map_err(|error| windows_io_context("validating the transaction parent", error))?;
    let actual_parent = normalize_windows_handle_path(
        windows_final_path(&parent_handle)
            .map_err(|error| windows_io_context("resolving the transaction parent", error))?
            .as_os_str(),
    );
    if actual_parent != expected_parent {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "transaction parent resolved through a junction: expected {expected_parent}, found {actual_parent}"
            ),
        ));
    }

    let source_handle = open_windows_rename_handle(source, DELETE | FILE_READ_ATTRIBUTES)
        .map_err(|error| windows_io_context("opening the transaction source", error))?;
    validate_windows_handle_type(&source_handle, directory)
        .map_err(|error| windows_io_context("validating the transaction source", error))?;
    let expected_source = normalize_windows_handle_path(source.as_os_str());
    let actual_source = normalize_windows_handle_path(
        windows_final_path(&source_handle)
            .map_err(|error| windows_io_context("resolving the transaction source", error))?
            .as_os_str(),
    );
    if actual_source != expected_source {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "transaction source resolved through a junction: expected {expected_source}, found {actual_source}"
            ),
        ));
    }

    // A simple basename lets FileRenameInfo keep the destination in the opened
    // source object's directory. An absolute path would reopen every lexical
    // ancestor and reintroduce a path-swap race after validation.
    let destination_name = destination_name.encode_wide().collect::<Vec<_>>();
    if destination_name.is_empty()
        || destination_name.len() > (u32::MAX as usize / std::mem::size_of::<u16>())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the transaction destination file name is invalid",
        ));
    }
    let header_size = std::mem::size_of::<FILE_RENAME_INFO>();
    let name_bytes = destination_name.len() * std::mem::size_of::<u16>();
    let buffer_size = header_size
        // Microsoft requires the buffer passed for FileRenameInfo to contain
        // sizeof(FILE_RENAME_INFO) *plus* the complete FileName byte count.
        // The structure's trailing FileName[1] must not be subtracted here:
        // some file-system drivers reject that shorter buffer before looking
        // at the otherwise valid rename request.
        .checked_add(name_bytes)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the transaction rename information is too large",
            )
        })?;
    let buffer_size = u32::try_from(buffer_size).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the transaction rename information exceeds the Win32 buffer limit",
        )
    })?;
    let words = (buffer_size as usize).div_ceil(std::mem::size_of::<usize>());
    let mut rename_buffer = vec![0_usize; words];
    let rename_info = rename_buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        // A NULL RootDirectory plus a simple name is the documented in-place
        // rename form: the destination remains in the verified source object's
        // current directory, without reopening a lexical ancestor. The source
        // handle denies delete sharing, and ReplaceIfExists = false remains a
        // kernel-enforced atomic no-replace operation. The corrected full
        // buffer size above is required even though FILE_RENAME_INFO declares
        // a trailing FileName[1]. WRITE_THROUGH is optional durability/cache
        // behavior rather than part of atomic no-replace semantics, so it is
        // omitted for cross-file-system compatibility. The durable transaction
        // journal provides recovery if a metadata update is interrupted.
        (*rename_info).Anonymous.ReplaceIfExists = false;
        (*rename_info).RootDirectory = std::ptr::null_mut();
        (*rename_info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            destination_name.as_ptr(),
            std::ptr::addr_of_mut!((*rename_info).FileName).cast::<u16>(),
            destination_name.len(),
        );
    }
    let renamed = unsafe {
        SetFileInformationByHandle(
            source_handle.as_raw_handle() as _,
            FileRenameInfo,
            rename_info.cast(),
            buffer_size,
        )
    };
    if renamed == 0 {
        return Err(windows_io_context(
            "renaming the transaction source with FileRenameInfo",
            std::io::Error::last_os_error(),
        ));
    }

    let expected_destination = normalize_windows_handle_path(destination.as_os_str());
    let actual_destination = normalize_windows_handle_path(
        windows_final_path(&source_handle)
            .map_err(|error| windows_io_context("resolving the renamed transaction source", error))?
            .as_os_str(),
    );
    if actual_destination != expected_destination {
        return Err(std::io::Error::other(format!(
                "renamed transaction handle resolved unexpectedly: expected {expected_destination}, found {actual_destination}"
            )));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_io_context(stage: &str, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("{stage} failed: {error}"))
}

#[cfg(target_os = "windows")]
fn open_windows_rename_handle(path: &Path, access: u32) -> std::io::Result<OwnedHandle> {
    let path = wide(path);
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) })
    }
}

#[cfg(target_os = "windows")]
fn validate_windows_handle_type(handle: &OwnedHandle, directory: bool) -> std::io::Result<()> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    let inspected =
        unsafe { GetFileInformationByHandle(handle.as_raw_handle() as _, &mut information) };
    if inspected == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let attributes = information.dwFileAttributes;
    let is_directory = attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || is_directory != directory {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a transaction rename handle changed type or is a reparse point",
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_final_path(handle: &OwnedHandle) -> std::io::Result<std::ffi::OsString> {
    let required = unsafe {
        GetFinalPathNameByHandleW(handle.as_raw_handle() as _, std::ptr::null_mut(), 0, 0)
    };
    if required == 0 {
        return Err(std::io::Error::last_os_error());
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
        return Err(std::io::Error::last_os_error());
    }
    path.truncate(written as usize);
    Ok(std::ffi::OsString::from_wide(&path))
}

#[cfg(target_os = "windows")]
fn normalize_windows_handle_path(path: &std::ffi::OsStr) -> String {
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

#[cfg(target_os = "linux")]
fn sync_directory(path: &Path) -> AppResult<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
fn sync_directory(path: &Path) -> AppResult<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn sync_directory(_path: &Path) -> AppResult<()> {
    // Windows does not provide a portable directory flush with Rust's file API.
    // The durable transaction journal is retained until all rename and state
    // operations complete, so startup recovery can reconcile an interrupted
    // metadata update without requiring optional write-through handle flags.
    Ok(())
}

#[cfg(target_os = "windows")]
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "linux")]
fn metadata_is_reparse(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(all(test, target_os = "macos"))]
fn metadata_is_reparse(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn metadata_is_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn recovery_io(message: &str, path: &Path, error: std::io::Error) -> InstallerError {
    InstallerError::with_detail(
        "transactionJournal",
        message,
        format!("{}: {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::os::unix::fs::symlink;

    fn state() -> ManagedState {
        ManagedState::default()
    }

    fn journal(before: String, after: Option<String>) -> PortableJournal {
        PortableJournal {
            schema_version: PORTABLE_JOURNAL_SCHEMA_VERSION,
            transaction_id: Uuid::new_v4().to_string(),
            installation_id: "linux-test".into(),
            quarantine_nonce: Uuid::new_v4().to_string(),
            phase: JournalPhase::Prepared,
            before_state_sha256: before,
            after_state_sha256: after,
            before_record: None,
            after_record: None,
            quarantines: Vec::new(),
            before_integration: Vec::new(),
            after_integration: Vec::new(),
            operation: JournalOperation::Uninstall(UninstallJournal {
                target: PathBuf::from("/tmp/Aseprite"),
                target_fingerprint: "a".repeat(64),
                backup: None,
                backup_fingerprint: None,
                before_integration_paths: Vec::new(),
                after_integration_paths: Vec::new(),
            }),
        }
    }

    #[test]
    fn registry_fingerprint_is_deterministic() {
        assert_eq!(
            managed_state_sha256(&state()).unwrap(),
            managed_state_sha256(&state()).unwrap()
        );
    }

    #[test]
    fn recovery_direction_ignores_a_stale_phase() {
        let before = managed_state_sha256(&state()).unwrap();
        let after = "b".repeat(64);
        let mut journal = journal(before.clone(), Some(after.clone()));
        journal.phase = JournalPhase::RegistryCommitted;
        assert_eq!(
            recovery_direction(&journal, &before).unwrap(),
            RecoveryDirection::BeforeCommit
        );
        journal.phase = JournalPhase::Prepared;
        assert_eq!(
            recovery_direction(&journal, &after).unwrap(),
            RecoveryDirection::AfterCommit
        );
        assert_eq!(
            recovery_direction(&journal, &"c".repeat(64))
                .unwrap_err()
                .code,
            "recoveryConflict"
        );
    }

    #[test]
    fn restore_journal_keeps_original_and_marker_adjusted_fingerprints_distinct() {
        let operation = RestoreJournal {
            target: PathBuf::from("/tmp/target"),
            target_staging: PathBuf::from("/tmp/target-staging"),
            target_previous: PathBuf::from("/tmp/target-previous"),
            backup: PathBuf::from("/tmp/backup"),
            backup_staging: PathBuf::from("/tmp/backup-staging"),
            backup_previous: PathBuf::from("/tmp/backup-previous"),
            current_fingerprint: "a".repeat(64),
            rollback_fingerprint: "b".repeat(64),
            restored_fingerprint: "c".repeat(64),
            before_integration_paths: vec![PathBuf::from("/tmp/old-profile/launcher")],
            after_integration_paths: vec![PathBuf::from("/tmp/new-profile/launcher")],
        };
        let encoded = serde_json::to_vec(&operation).unwrap();
        let decoded: RestoreJournal = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.rollback_fingerprint, "b".repeat(64));
        assert_eq!(decoded.restored_fingerprint, "c".repeat(64));
        assert_ne!(decoded.rollback_fingerprint, decoded.restored_fingerprint);
        assert_ne!(
            decoded.before_integration_paths,
            decoded.after_integration_paths
        );
    }

    #[test]
    fn no_replace_rename_refuses_an_occupied_destination() {
        let directory = tempfile::tempdir().unwrap();
        let directory_path = std::fs::canonicalize(directory.path()).unwrap();
        let source = directory_path.join("source");
        let destination = directory_path.join("destination");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&destination).unwrap();
        assert!(durable_rename_no_replace(&source, &destination).is_err());
        assert!(source.exists());
        assert!(destination.exists());
    }

    #[test]
    fn no_replace_file_rename_moves_once_and_refuses_a_collision() {
        let directory = tempfile::tempdir().unwrap();
        let directory_path = std::fs::canonicalize(directory.path()).unwrap();
        let source = directory_path.join("source.json");
        let destination = directory_path.join("destination.json");
        std::fs::write(&source, b"transaction proof").unwrap();
        durable_rename_file_no_replace(&source, &destination).unwrap();
        assert!(!source.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"transaction proof");

        let second_source = directory_path.join("second.json");
        std::fs::write(&second_source, b"must survive").unwrap();
        let collision = durable_rename_file_no_replace(&second_source, &destination).unwrap_err();
        assert_eq!(collision.code, "transactionCollision");
        assert_eq!(std::fs::read(&second_source).unwrap(), b"must survive");
        assert_eq!(std::fs::read(&destination).unwrap(), b"transaction proof");
    }

    #[test]
    fn journal_round_trip_and_future_schema_rejection() {
        let directory = tempfile::tempdir().unwrap();
        let paths = InstallerPaths::new(
            directory.path().join("data"),
            directory.path().join("cache"),
        );
        let before = managed_state_sha256(&state()).unwrap();
        let transaction = journal(before, Some("b".repeat(64)));
        write_journal(&paths, &transaction).unwrap();
        sync_tree_durable(&paths.data_dir).unwrap();
        let loaded = load_journal(&paths).unwrap().unwrap();
        assert_eq!(loaded.transaction_id, transaction.transaction_id);
        remove_journal(&paths).unwrap();
        assert!(load_journal(&paths).unwrap().is_none());

        let mut future = transaction;
        future.schema_version = PORTABLE_JOURNAL_SCHEMA_VERSION + 1;
        assert_eq!(
            write_journal(&paths, &future).unwrap_err().code,
            "recoveryConflict"
        );
    }

    #[test]
    fn integration_snapshot_digest_detects_tampering() {
        let absent = IntegrationSnapshot::absent(PathBuf::from("/tmp/missing.desktop"));
        absent.validate().unwrap();
        let absent_round_trip: IntegrationSnapshot =
            serde_json::from_slice(&serde_json::to_vec(&absent).unwrap()).unwrap();
        assert_eq!(absent_round_trip, absent);

        let mut snapshot = IntegrationSnapshot::file(
            PathBuf::from("/tmp/aseprite.desktop"),
            b"owned launcher".to_vec(),
            Some(0o644),
        )
        .unwrap();
        snapshot.validate().unwrap();
        snapshot.contents.as_mut().unwrap()[0] ^= 0xff;
        assert_eq!(snapshot.validate().unwrap_err().code, "recoveryConflict");
    }

    #[test]
    fn journal_rejects_invalid_quarantine_nonce_and_fingerprint() {
        let before = managed_state_sha256(&state()).unwrap();
        let mut transaction = journal(before, None);
        transaction.quarantine_nonce = "forged".into();
        assert_eq!(
            validate_journal_header(&transaction).unwrap_err().code,
            "recoveryConflict"
        );
        transaction.quarantine_nonce = Uuid::new_v4().to_string();
        transaction.quarantines.push(QuarantineEntry {
            source: PathBuf::from("/tmp/Aseprite"),
            quarantine: PathBuf::from("/tmp/.aseprite-quarantine-test"),
            fingerprint: "short".into(),
        });
        assert_eq!(
            validate_journal_header(&transaction).unwrap_err().code,
            "recoveryConflict"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn journal_loader_refuses_a_symbolic_link() {
        let directory = tempfile::tempdir().unwrap();
        let paths = InstallerPaths::new(
            directory.path().join("data"),
            directory.path().join("cache"),
        );
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        let target = directory.path().join("other.json");
        std::fs::write(&target, b"{}").unwrap();
        symlink(&target, &paths.transaction_file).unwrap();
        assert!(load_journal(&paths).is_err());
    }
}
