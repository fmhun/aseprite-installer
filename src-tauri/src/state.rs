use crate::error::{AppResult, InstallerError};
use crate::models::ManagedState;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

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
    pub client: reqwest::Client,
    active_operation: Mutex<Option<Arc<AtomicBool>>>,
}

impl AppState {
    pub fn new(paths: InstallerPaths) -> AppResult<Self> {
        paths.ensure()?;
        let client = reqwest::Client::builder()
            .user_agent("fmhun/aseprite-installer")
            .https_only(true)
            .build()
            .map_err(InstallerError::from)?;
        Ok(Self {
            paths,
            client,
            active_operation: Mutex::new(None),
        })
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
        let cancelled = Arc::new(AtomicBool::new(false));
        *active = Some(cancelled.clone());
        Ok(cancelled)
    }

    pub fn cancel_operation(&self) -> AppResult<()> {
        let active = self
            .active_operation
            .lock()
            .map_err(|_| InstallerError::new("state", "Installer state is unavailable."))?;
        if let Some(cancelled) = active.as_ref() {
            cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
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
}

pub fn load_managed_state(path: &Path) -> AppResult<ManagedState> {
    if !path.exists() {
        return Ok(ManagedState::default());
    }
    let contents = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&contents)?)
}

pub fn save_managed_state(path: &Path, managed: &ManagedState) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| InstallerError::new("path", "Registry path has no parent."))?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(managed)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_managed_state_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let state = ManagedState::default();
        save_managed_state(&path, &state).unwrap();
        assert_eq!(load_managed_state(&path).unwrap().schema_version, 1);
        assert!(!path.with_extension("json.tmp").exists());
    }
}
