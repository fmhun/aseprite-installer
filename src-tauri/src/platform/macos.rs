use super::PlatformAdapter;
use crate::error::{AppResult, InstallerError};
use crate::models::{
    InstallationChannel, InstallationInfo, ManagedState, PreflightReport, Prerequisite,
};
use crate::state::InstallerPaths;
use async_trait::async_trait;
use fs2::available_space;
use plist::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

const BUNDLE_IDENTIFIER: &str = "org.aseprite.Aseprite";
const MINIMUM_FREE_BYTES: u64 = 6 * 1024 * 1024 * 1024;

pub struct MacOsAdapter;

impl MacOsAdapter {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PlatformAdapter for MacOsAdapter {
    async fn discover_installations(
        &self,
        paths: &InstallerPaths,
        managed: &ManagedState,
    ) -> AppResult<Vec<InstallationInfo>> {
        let paths = paths.clone();
        let managed = managed.clone();
        tauri::async_runtime::spawn_blocking(move || discover(&paths, &managed))
            .await
            .map_err(|error| {
                InstallerError::with_detail(
                    "scan",
                    "Aseprite installations could not be scanned.",
                    error.to_string(),
                )
            })?
    }

    async fn preflight(&self, paths: &InstallerPaths) -> AppResult<PreflightReport> {
        let paths = paths.clone();
        tauri::async_runtime::spawn_blocking(move || run_preflight(&paths))
            .await
            .map_err(|error| {
                InstallerError::with_detail(
                    "preflight",
                    "The Mac build environment could not be checked.",
                    error.to_string(),
                )
            })?
    }

    fn default_target(&self) -> AppResult<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| {
            InstallerError::new("home", "The current user home directory is unavailable.")
        })?;
        Ok(home.join("Applications").join("Aseprite.app"))
    }
}

fn discover(_paths: &InstallerPaths, managed: &ManagedState) -> AppResult<Vec<InstallationInfo>> {
    let home = dirs::home_dir().ok_or_else(|| {
        InstallerError::new("home", "The current user home directory is unavailable.")
    })?;
    let mut candidates = BTreeSet::new();
    candidates.insert(home.join("Applications").join("Aseprite.app"));
    candidates.insert(PathBuf::from("/Applications/Aseprite.app"));
    candidates.insert(
        home.join("Library/Application Support/Steam/steamapps/common/Aseprite/Aseprite.app"),
    );
    candidates.insert(PathBuf::from(
        "/opt/homebrew/Caskroom/aseprite/current/Aseprite.app",
    ));
    candidates.insert(PathBuf::from(
        "/usr/local/Caskroom/aseprite/current/Aseprite.app",
    ));

    if let Ok(output) = Command::new("/usr/bin/mdfind")
        .arg("kMDItemCFBundleIdentifier == 'org.aseprite.Aseprite'")
        .output()
    {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let path = PathBuf::from(line.trim());
                if path.extension().and_then(|extension| extension.to_str()) == Some("app") {
                    candidates.insert(path);
                }
            }
        }
    }

    let steam_common = home.join("Library/Application Support/Steam/steamapps/common");
    collect_named_apps(&steam_common, &mut candidates, 4);
    collect_named_apps(
        Path::new("/opt/homebrew/Caskroom/aseprite"),
        &mut candidates,
        5,
    );
    collect_named_apps(
        Path::new("/usr/local/Caskroom/aseprite"),
        &mut candidates,
        5,
    );

    for record in &managed.installations {
        candidates.insert(PathBuf::from(&record.path));
    }

    let mut results = Vec::new();
    let mut seen = BTreeSet::new();
    for candidate in candidates {
        if !candidate.exists() || !is_aseprite_bundle(&candidate) {
            continue;
        }
        let normalized = std::fs::canonicalize(&candidate).unwrap_or(candidate.clone());
        if !seen.insert(normalized.clone()) {
            continue;
        }
        let path_string = normalized.to_string_lossy().into_owned();
        let record = managed
            .installations
            .iter()
            .find(|record| paths_equal(Path::new(&record.path), &normalized));
        let channel = if record.is_some() {
            InstallationChannel::Managed
        } else {
            infer_channel(&normalized)
        };
        let bundle_version = bundle_string(&normalized, "CFBundleShortVersionString");
        let version = record.map(|record| record.tag.clone()).or(bundle_version);
        let writable = normalized
            .parent()
            .and_then(|parent| std::fs::metadata(parent).ok())
            .map(|metadata| !metadata.permissions().readonly())
            .unwrap_or(false);
        let manageable = channel == InstallationChannel::Managed
            || (channel == InstallationChannel::Manual && writable);
        let id = installation_id(&path_string);
        let backup = record
            .and_then(|record| record.backup_path.as_ref())
            .map(PathBuf::from)
            .filter(|path| path.exists());
        results.push(InstallationInfo {
            id,
            path: path_string,
            version,
            version_exact: record.map(|record| record.version_exact).unwrap_or(false),
            architecture: bundle_architecture(&normalized),
            channel,
            manageable,
            writable,
            has_backup: backup.is_some(),
            installed_at: record.map(|record| record.installed_at.clone()),
        });
    }
    results.sort_by_key(|installation| match installation.channel {
        InstallationChannel::Managed => 0,
        InstallationChannel::Manual => 1,
        InstallationChannel::Steam => 2,
        InstallationChannel::PackageManager => 3,
    });
    Ok(results)
}

fn collect_named_apps(root: &Path, candidates: &mut BTreeSet<PathBuf>, depth: usize) {
    if !root.exists() {
        return;
    }
    for entry in WalkDir::new(root)
        .max_depth(depth)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_dir()
            && entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("Aseprite.app")
        {
            candidates.insert(entry.path().to_path_buf());
        }
    }
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf())
        == std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf())
}

pub fn is_aseprite_bundle(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("app")
        && bundle_string(path, "CFBundleIdentifier").as_deref() == Some(BUNDLE_IDENTIFIER)
}

fn bundle_string(path: &Path, key: &str) -> Option<String> {
    let plist = Value::from_file(path.join("Contents/Info.plist")).ok()?;
    plist
        .as_dictionary()?
        .get(key)?
        .as_string()
        .map(str::to_owned)
}

fn bundle_architecture(path: &Path) -> Option<String> {
    let executable = bundle_string(path, "CFBundleExecutable")?;
    let executable = path.join("Contents/MacOS").join(executable);
    let output = Command::new("/usr/bin/lipo")
        .arg("-archs")
        .arg(executable)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn infer_channel(path: &Path) -> InstallationChannel {
    let lowercase = path.to_string_lossy().to_lowercase();
    if lowercase.contains("/steamapps/") {
        InstallationChannel::Steam
    } else if lowercase.contains("/caskroom/")
        || lowercase.contains("/cellar/")
        || lowercase.contains("/macports/")
    {
        InstallationChannel::PackageManager
    } else {
        InstallationChannel::Manual
    }
}

pub fn installation_id(path: &str) -> String {
    let digest = Sha256::digest(path.as_bytes());
    format!("aseprite-{}", &hex::encode(digest)[..16])
}

fn run_preflight(paths: &InstallerPaths) -> AppResult<PreflightReport> {
    paths.ensure()?;
    let os_version = command_output("/usr/bin/sw_vers", &["-productVersion"])
        .unwrap_or_else(|| "Unknown".into());
    let architecture = std::env::consts::ARCH.to_owned();
    let supported_os = os_version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .map(|major| major >= 15)
        .unwrap_or(false);
    let xcode_path = command_output("/usr/bin/xcode-select", &["-p"]);
    let sdk_path = command_output("/usr/bin/xcrun", &["--sdk", "macosx", "--show-sdk-path"]);
    let clang = command_output("/usr/bin/xcrun", &["clang", "--version"]);
    let cmake = find_executable("cmake");
    let ninja = find_executable("ninja");
    let homebrew = find_executable("brew");
    let free_bytes = available_space(&paths.cache_dir).unwrap_or(0);

    let prerequisites = vec![
        Prerequisite {
            id: "macos".into(),
            label: "macOS".into(),
            ok: supported_os,
            required: true,
            detail: os_version.clone(),
            remediation: (!supported_os).then(|| "macOS 15.2 or newer is required.".into()),
        },
        Prerequisite {
            id: "xcode".into(),
            label: "Xcode".into(),
            ok: xcode_path.is_some() && clang.is_some(),
            required: true,
            detail: xcode_path.unwrap_or_else(|| "Not configured".into()),
            remediation: Some(
                "Install Xcode, open it once, then select it with xcode-select.".into(),
            ),
        },
        Prerequisite {
            id: "sdk".into(),
            label: "macOS SDK".into(),
            ok: sdk_path.is_some(),
            required: true,
            detail: sdk_path.unwrap_or_else(|| "Not found".into()),
            remediation: Some("Install the macOS SDK included with Xcode.".into()),
        },
        Prerequisite {
            id: "cmake".into(),
            label: "CMake".into(),
            ok: cmake.is_some(),
            required: true,
            detail: cmake.unwrap_or_else(|| "Not found".into()),
            remediation: Some("Install CMake from cmake.org or Homebrew.".into()),
        },
        Prerequisite {
            id: "ninja".into(),
            label: "Ninja".into(),
            ok: ninja.is_some(),
            required: true,
            detail: ninja.unwrap_or_else(|| "Not found".into()),
            remediation: Some("Install Ninja from ninja-build.org or Homebrew.".into()),
        },
        Prerequisite {
            id: "disk".into(),
            label: "Free disk space".into(),
            ok: free_bytes >= MINIMUM_FREE_BYTES,
            required: true,
            detail: format!("{:.1} GB", free_bytes as f64 / 1024_f64.powi(3)),
            remediation: Some("Free at least 6 GB before compiling.".into()),
        },
    ];
    let ready = prerequisites.iter().all(|item| !item.required || item.ok);
    Ok(PreflightReport {
        ready,
        architecture,
        os_version,
        free_bytes,
        minimum_free_bytes: MINIMUM_FREE_BYTES,
        homebrew_available: homebrew.is_some(),
        prerequisites,
    })
}

pub fn find_executable(name: &str) -> Option<String> {
    let candidates = [
        PathBuf::from("/opt/homebrew/bin").join(name),
        PathBuf::from("/usr/local/bin").join(name),
        PathBuf::from("/usr/bin").join(name),
        PathBuf::from("/bin").join(name),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_channels_without_guessing_browser_origin() {
        assert_eq!(
            infer_channel(Path::new(
                "/Users/test/Library/Application Support/Steam/steamapps/common/Aseprite/Aseprite.app"
            )),
            InstallationChannel::Steam
        );
        assert_eq!(
            infer_channel(Path::new("/Applications/Aseprite.app")),
            InstallationChannel::Manual
        );
    }

    #[test]
    fn stable_ids_are_path_specific() {
        let first = installation_id("/Applications/Aseprite.app");
        assert_eq!(first, installation_id("/Applications/Aseprite.app"));
        assert_ne!(
            first,
            installation_id("/Users/test/Applications/Aseprite.app")
        );
    }
}
