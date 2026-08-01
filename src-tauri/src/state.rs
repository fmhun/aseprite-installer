use crate::error::{AppResult, InstallerError};
use crate::models::{ManagedState, RecoveryStatus, MANAGED_STATE_SCHEMA_VERSION};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;
use uuid::Uuid;

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

#[derive(Debug, Clone)]
pub struct InstallerPaths {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub registry_file: PathBuf,
    #[cfg(any(target_os = "linux", target_os = "windows", test))]
    pub transaction_file: PathBuf,
    pub archives_dir: PathBuf,
    pub builds_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub backups_dir: PathBuf,
}

impl InstallerPaths {
    pub fn new(data_dir: PathBuf, cache_dir: PathBuf) -> Self {
        Self {
            registry_file: data_dir.join("managed-state.json"),
            #[cfg(any(target_os = "linux", target_os = "windows", test))]
            transaction_file: data_dir.join("portable-transaction.json"),
            archives_dir: cache_dir.join("archives"),
            builds_dir: cache_dir.join("builds"),
            logs_dir: data_dir.join("logs"),
            backups_dir: data_dir.join("backups"),
            data_dir,
            cache_dir,
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub fn ensure(&self) -> AppResult<()> {
        for directory in [
            &self.data_dir,
            &self.cache_dir,
            &self.archives_dir,
            &self.builds_dir,
            &self.logs_dir,
            &self.backups_dir,
        ] {
            std::fs::create_dir_all(directory)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct AppState {
    pub paths: InstallerPaths,
    active_operation: Arc<Mutex<Option<ActiveOperation>>>,
    recovery_error: Arc<Mutex<Option<InstallerError>>>,
}

struct ActiveOperation {
    cancelled: Arc<AtomicBool>,
    _interprocess_lock: File,
}

pub struct RegistryLock(File);

#[derive(Debug)]
pub struct ObservationLock(File);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommitDurability {
    Durable,
    Uncertain(String),
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

impl Drop for ObservationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

impl AppState {
    pub fn new(paths: InstallerPaths) -> AppResult<Self> {
        // Validate the HTTP configuration without retaining a snapshot of the
        // macOS proxy/keychain settings for the lifetime of the app.
        build_http_client()?;
        Ok(Self {
            paths,
            active_operation: Arc::new(Mutex::new(None)),
            recovery_error: Arc::new(Mutex::new(None)),
        })
    }

    pub fn http_client(&self) -> AppResult<reqwest::Client> {
        build_http_client()
    }

    pub fn begin_operation(&self) -> AppResult<Arc<AtomicBool>> {
        let mut active = self
            .active_operation
            .lock()
            .map_err(|_| InstallerError::new("state", "Installer state is unavailable."))?;
        if active.is_some() {
            return Err(InstallerError::new(
                "busy",
                "Another installer operation is already running.",
            ));
        }
        std::fs::create_dir_all(&self.paths.data_dir)?;
        let lock_path = self.paths.data_dir.join(".operation.lock");
        let interprocess_lock = open_lock_file(&lock_path)?;
        interprocess_lock.try_lock_exclusive().map_err(|error| {
            InstallerError::with_detail(
                "busy",
                "Another Aseprite Installer process is already changing files.",
                format!("{}: {error}", lock_path.display()),
            )
        })?;
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Err(error) = crate::installer::recover_pending_transaction(self) {
            self.set_recovery_error(Some(error.clone()));
            return Err(error);
        }
        self.set_recovery_error(None);
        let cancelled = Arc::new(AtomicBool::new(false));
        *active = Some(ActiveOperation {
            cancelled: cancelled.clone(),
            _interprocess_lock: interprocess_lock,
        });
        Ok(cancelled)
    }

    pub fn cancel_operation(&self) -> AppResult<()> {
        let active = self
            .active_operation
            .lock()
            .map_err(|_| InstallerError::new("state", "Installer state is unavailable."))?;
        if let Some(active) = active.as_ref() {
            active
                .cancelled
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }

    pub fn finish_operation(&self) {
        if let Ok(mut active) = self.active_operation.lock() {
            *active = None;
        }
    }

    pub fn begin_observation(&self) -> AppResult<ObservationLock> {
        if self
            .active_operation
            .lock()
            .map_err(|_| InstallerError::new("state", "Installer state is unavailable."))?
            .is_some()
        {
            return Err(InstallerError::new(
                "busy",
                "Another installer operation is already running.",
            ));
        }
        self.ensure_recovery_ready()?;
        std::fs::create_dir_all(&self.paths.data_dir)?;
        let lock_path = self.paths.data_dir.join(".operation.lock");
        let file = open_lock_file(&lock_path)?;
        FileExt::try_lock_shared(&file).map_err(|error| {
            InstallerError::with_detail(
                "busy",
                "Another Aseprite Installer process is changing files.",
                format!("{}: {error}", lock_path.display()),
            )
        })?;
        if self
            .active_operation
            .lock()
            .map_err(|_| InstallerError::new("state", "Installer state is unavailable."))?
            .is_some()
        {
            let _ = FileExt::unlock(&file);
            return Err(InstallerError::new(
                "busy",
                "Another installer operation is already running.",
            ));
        }
        if let Err(error) = self.ensure_recovery_ready() {
            let _ = FileExt::unlock(&file);
            return Err(error);
        }
        Ok(ObservationLock(file))
    }

    pub fn recovery_status(&self) -> RecoveryStatus {
        let stored = self
            .recovery_error
            .lock()
            .ok()
            .and_then(|error| error.clone());
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let journal_pending = std::fs::symlink_metadata(&self.paths.transaction_file).is_ok();
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let journal_pending = false;
        let blocked = stored.is_some() || journal_pending;
        let message = stored
            .as_ref()
            .map(|error| error.message.clone())
            .or_else(|| {
                journal_pending.then(|| {
                    "An interrupted installation must be recovered before Aseprite can be changed or launched.".into()
                })
            });
        let detail = stored.as_ref().and_then(|error| error.detail.clone());
        RecoveryStatus {
            blocked,
            message,
            detail,
            journal_path: blocked.then(|| {
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                {
                    self.paths.transaction_file.to_string_lossy().into_owned()
                }
                #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                {
                    String::new()
                }
            }),
        }
    }

    pub fn ensure_recovery_ready(&self) -> AppResult<()> {
        let status = self.recovery_status();
        if !status.blocked {
            return Ok(());
        }
        Err(InstallerError::with_detail(
            "recoveryBlocked",
            status.message.unwrap_or_else(|| {
                "An interrupted installation must be recovered before continuing.".into()
            }),
            status
                .journal_path
                .unwrap_or_else(|| self.paths.data_dir.display().to_string()),
        ))
    }

    pub fn set_recovery_error(&self, error: Option<InstallerError>) {
        if let Ok(mut stored) = self.recovery_error.lock() {
            *stored = error;
        }
    }

    pub fn load_managed_state(&self) -> AppResult<ManagedState> {
        load_managed_state(&self.paths.registry_file)
    }

    #[cfg(target_os = "macos")]
    pub fn save_managed_state(&self, managed: &ManagedState) -> AppResult<()> {
        save_managed_state(&self.paths.registry_file, managed)
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub(crate) fn save_managed_state_transactional(
        &self,
        managed: &ManagedState,
    ) -> AppResult<CommitDurability> {
        save_managed_state_with_durability(&self.paths.registry_file, managed)
    }

    pub fn lock_registry(&self) -> AppResult<RegistryLock> {
        std::fs::create_dir_all(&self.paths.data_dir)?;
        let lock_path = self.paths.data_dir.join(".managed-state.lock");
        let file = open_lock_file(&lock_path)?;
        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(RegistryLock(file)),
                Err(_) if started.elapsed() < Duration::from_secs(10) => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => {
                    return Err(InstallerError::with_detail(
                        "registryBusy",
                        "Another installer process is updating managed installations.",
                        format!("{}: {error}", lock_path.display()),
                    ));
                }
            }
        }
    }
}

fn build_http_client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("fmhun/aseprite-installer")
        .https_only(true)
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .read_timeout(HTTP_READ_TIMEOUT)
        .build()
        .map_err(InstallerError::from)
}

pub fn load_managed_state(path: &Path) -> AppResult<ManagedState> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManagedState::default());
        }
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata_is_reparse_point(&metadata) {
        return Err(InstallerError::with_detail(
            "registryType",
            "The managed-installation registry is not a regular non-reparse file.",
            path.display().to_string(),
        ));
    }
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let managed: ManagedState = serde_json::from_str(&contents)?;
    if managed.schema_version == 0 || managed.schema_version > MANAGED_STATE_SCHEMA_VERSION {
        return Err(InstallerError::with_detail(
            "stateSchema",
            "This managed-installation registry uses an unsupported schema version.",
            format!(
                "Found schema version {}; this build supports versions 1 through {}. Update Aseprite Installer before changing managed installations.",
                managed.schema_version, MANAGED_STATE_SCHEMA_VERSION
            ),
        ));
    }
    Ok(managed)
}

pub(crate) fn open_lock_file(path: &Path) -> AppResult<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path).map_err(|error| {
        InstallerError::with_detail(
            "lockFile",
            "An installer lock file could not be opened safely.",
            format!("{}: {error}", path.display()),
        )
    })?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata_is_reparse_point(&metadata) {
        return Err(InstallerError::with_detail(
            "lockFileType",
            "An installer lock path is not a regular non-reparse file.",
            path.display().to_string(),
        ));
    }
    Ok(file)
}

#[cfg(any(target_os = "macos", test))]
pub fn save_managed_state(path: &Path, managed: &ManagedState) -> AppResult<()> {
    match save_managed_state_with_durability(path, managed)? {
        CommitDurability::Durable => {}
        CommitDurability::Uncertain(detail) => eprintln!(
            "Aseprite Installer committed {} but could not prove it durable: {detail}",
            path.display()
        ),
    }
    Ok(())
}

fn save_managed_state_with_durability(
    path: &Path,
    managed: &ManagedState,
) -> AppResult<CommitDurability> {
    if managed.schema_version == 0 || managed.schema_version > MANAGED_STATE_SCHEMA_VERSION {
        return Err(InstallerError::with_detail(
            "stateSchema",
            "Refusing to overwrite a managed-installation registry with an unsupported schema version.",
            format!(
                "Schema version {}; supported versions are 1 through {}.",
                managed.schema_version, MANAGED_STATE_SCHEMA_VERSION
            ),
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| InstallerError::new("path", "Registry path has no parent."))?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(managed)?)?;
        file.sync_all()?;
        drop(file);
        let durability = replace_file_durable(&temporary, path)?;
        Ok::<CommitDurability, InstallerError>(durability)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    let durability = result?;
    if matches!(&durability, CommitDurability::Uncertain(_)) {
        return Ok(durability);
    }
    // The atomic replacement is the logical commit boundary. A directory fsync failure
    // after this point must not make callers roll the application back while
    // the registry already contains the new state.
    #[cfg(not(windows))]
    if let Err(error) = File::open(parent).and_then(|directory| directory.sync_all()) {
        return Ok(CommitDurability::Uncertain(format!(
            "The registry replacement succeeded, but its parent directory could not be synchronized: {error}"
        )));
    }
    Ok(CommitDurability::Durable)
}

#[cfg(not(windows))]
pub(crate) fn replace_file_durable(
    temporary: &Path,
    destination: &Path,
) -> std::io::Result<CommitDurability> {
    std::fs::rename(temporary, destination)?;
    Ok(CommitDurability::Durable)
}

#[cfg(windows)]
pub(crate) fn replace_file_durable(
    temporary: &Path,
    destination: &Path,
) -> std::io::Result<CommitDurability> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};

    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    #[link(name = "Kernel32")]
    extern "system" {
        fn ReplaceFileW(
            replaced_file_name: *const u16,
            replacement_file_name: *const u16,
            backup_file_name: *const u16,
            replace_flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    };
    let destination_exists = match std::fs::symlink_metadata(destination) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 =>
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the registry destination is not a regular file",
            ));
        }
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let destination_path = destination;
    let temporary = wide(temporary);
    let destination = wide(destination_path);

    let succeeded = unsafe {
        if destination_exists {
            ReplaceFileW(
                destination.as_ptr(),
                temporary.as_ptr(),
                null(),
                0,
                null_mut(),
                null_mut(),
            )
        } else {
            MoveFileExW(
                temporary.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if destination_exists {
        let persistence = (|| {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .write(true)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
            let destination = options.open(destination_path)?;
            let metadata = destination.metadata()?;
            if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the replaced registry destination is not a regular non-reparse file",
                ));
            }
            destination.sync_all()
        })();
        // ReplaceFileW already crossed the logical commit boundary. Reporting an
        // error here would make callers roll files back while the new registry is
        // visible. The durable transaction journal resolves the actual registry
        // side after a power loss.
        if let Err(error) = persistence {
            return Ok(CommitDurability::Uncertain(format!(
                "The replacement succeeded, but {} could not be flushed: {error}",
                destination_path.display()
            )));
        }
    }
    Ok(CommitDurability::Durable)
}

#[cfg(windows)]
fn metadata_is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[test]
    fn startup_does_not_mutate_storage_before_permissions_are_checked() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("missing-data");
        let cache = directory.path().join("missing-cache");
        AppState::new(InstallerPaths::new(data.clone(), cache.clone())).unwrap();
        assert!(!data.exists());
        assert!(!cache.exists());
    }

    #[test]
    fn round_trips_managed_state_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let state = ManagedState::default();
        save_managed_state(&path, &state).unwrap();
        assert_eq!(load_managed_state(&path).unwrap().schema_version, 2);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn loads_registry_records_created_before_source_versions_were_stored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "schemaVersion": 1,
                "installations": [{
                    "id": "legacy",
                    "path": "/Users/test/Applications/Aseprite.app",
                    "tag": "v1.3.14.2",
                    "versionExact": true,
                    "digest": format!("sha256:{}", "a".repeat(64)),
                    "architecture": "arm64",
                    "installedAt": "2026-01-01T00:00:00Z",
                    "backupPath": null,
                    "backupTag": null,
                    "backupDigest": null,
                    "backupInstalledAt": null,
                    "backupVersionExact": null
                }]
            })
            .to_string(),
        )
        .unwrap();

        let state = load_managed_state(&path).unwrap();
        assert_eq!(state.schema_version, 1);
        assert_eq!(state.installations[0].source_version, None);
        assert_eq!(state.installations[0].backup_source_version, None);
        assert_eq!(state.installations[0].bundle_fingerprint, None);
        assert_eq!(state.installations[0].backup_bundle_fingerprint, None);
        assert_eq!(state.installations[0].backup_architecture, None);
    }

    #[test]
    fn refuses_future_registry_schema_without_overwriting_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let contents = serde_json::json!({
            "schemaVersion": MANAGED_STATE_SCHEMA_VERSION + 1,
            "installations": []
        })
        .to_string();
        std::fs::write(&path, &contents).unwrap();

        let error = load_managed_state(&path).unwrap_err();
        assert_eq!(error.code, "stateSchema");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    }

    #[test]
    fn refuses_to_save_future_registry_schema() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let state = ManagedState {
            schema_version: MANAGED_STATE_SCHEMA_VERSION + 1,
            installations: Vec::new(),
        };

        let error = save_managed_state(&path, &state).unwrap_err();
        assert_eq!(error.code, "stateSchema");
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_registry_and_lock_paths() {
        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real-file");
        std::fs::write(&real, serde_json::to_vec(&ManagedState::default()).unwrap()).unwrap();

        let registry = directory.path().join("managed-state.json");
        symlink(&real, &registry).unwrap();
        assert!(load_managed_state(&registry).is_err());

        let lock = directory.path().join(".operation.lock");
        symlink(&real, &lock).unwrap();
        assert!(open_lock_file(&lock).is_err());
    }

    #[test]
    fn operation_lock_excludes_a_second_app_state() {
        let directory = tempfile::tempdir().unwrap();
        let paths = InstallerPaths::new(
            directory.path().join("data"),
            directory.path().join("cache"),
        );
        let first = AppState::new(paths.clone()).unwrap();
        let second = AppState::new(paths).unwrap();
        first.begin_operation().unwrap();
        assert_eq!(second.begin_operation().unwrap_err().code, "busy");
        first.finish_operation();
        second.begin_operation().unwrap();
    }

    #[test]
    fn observation_lock_and_mutation_lock_exclude_each_other() {
        let directory = tempfile::tempdir().unwrap();
        let paths = InstallerPaths::new(
            directory.path().join("data"),
            directory.path().join("cache"),
        );
        let observing = AppState::new(paths.clone()).unwrap();
        let mutating = AppState::new(paths).unwrap();

        let observation = observing.begin_observation().unwrap();
        assert_eq!(mutating.begin_operation().unwrap_err().code, "busy");
        drop(observation);

        mutating.begin_operation().unwrap();
        assert_eq!(observing.begin_observation().unwrap_err().code, "busy");
        mutating.finish_operation();
        observing.begin_observation().unwrap();
    }
}
