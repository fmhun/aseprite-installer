use crate::error::{AppResult, InstallerError};
use crate::models::ManagedState;
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;
use uuid::Uuid;

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct InstallerPaths {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub registry_file: PathBuf,
    pub archives_dir: PathBuf,
    pub builds_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub backups_dir: PathBuf,
}

impl InstallerPaths {
    pub fn new(data_dir: PathBuf, cache_dir: PathBuf) -> Self {
        Self {
            registry_file: data_dir.join("managed-state.json"),
            archives_dir: cache_dir.join("archives"),
            builds_dir: cache_dir.join("builds"),
            logs_dir: data_dir.join("logs"),
            backups_dir: data_dir.join("backups"),
            data_dir,
            cache_dir,
        }
    }

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

pub struct AppState {
    pub paths: InstallerPaths,
    active_operation: Mutex<Option<ActiveOperation>>,
}

struct ActiveOperation {
    cancelled: Arc<AtomicBool>,
    _interprocess_lock: File,
}

pub struct RegistryLock(File);

impl Drop for RegistryLock {
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
            active_operation: Mutex::new(None),
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

    pub fn load_managed_state(&self) -> AppResult<ManagedState> {
        load_managed_state(&self.paths.registry_file)
    }

    pub fn save_managed_state(&self, managed: &ManagedState) -> AppResult<()> {
        save_managed_state(&self.paths.registry_file, managed)
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
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManagedState::default());
        }
        Err(error) => return Err(error.into()),
    };
    if !file.metadata()?.is_file() {
        return Err(InstallerError::with_detail(
            "registryType",
            "The managed-installation registry is not a regular file.",
            path.display().to_string(),
        ));
    }
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(serde_json::from_str(&contents)?)
}

pub(crate) fn open_lock_file(path: &Path) -> AppResult<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            InstallerError::with_detail(
                "lockFile",
                "An installer lock file could not be opened safely.",
                format!("{}: {error}", path.display()),
            )
        })?;
    if !file.metadata()?.is_file() {
        return Err(InstallerError::with_detail(
            "lockFileType",
            "An installer lock path is not a regular file.",
            path.display().to_string(),
        ));
    }
    Ok(file)
}

pub fn save_managed_state(path: &Path, managed: &ManagedState) -> AppResult<()> {
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
        std::fs::rename(&temporary, path)?;
        Ok::<(), InstallerError>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    // The rename is the logical commit boundary. A directory fsync failure
    // after this point must not make callers roll the application back while
    // the registry already contains the new state.
    if let Err(error) = File::open(parent).and_then(|directory| directory.sync_all()) {
        eprintln!(
            "Aseprite Installer committed {} but could not fsync its parent directory: {error}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
