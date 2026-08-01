use super::{PlatformAdapter, PreflightContext};
use crate::error::{AppResult, InstallerError};
use crate::models::{
    InstallationChannel, InstallationInfo, ManagedState, PreflightReport, Prerequisite,
};
use crate::state::{open_lock_file, InstallerPaths};
use async_trait::async_trait;
use fs2::{available_space, FileExt};
use plist::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CString, OsStr, OsString};
use std::io::{Read, Write};
use std::os::macos::fs::MetadataExt as MacOsMetadataExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use url::Url;
use uuid::Uuid;
use walkdir::WalkDir;

const BUNDLE_IDENTIFIER: &str = "org.aseprite.Aseprite";
const BUILD_SAFETY_BUDGET_BYTES: u64 = 6 * 1024 * 1024 * 1024;
const COMMAND_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const SKIA_ROUTE_PROBE_TAG: &str = "m124-08a5439a6b";
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
const UF_IMMUTABLE: u32 = 0x0000_0002;
const UF_APPEND: u32 = 0x0000_0004;
const SF_IMMUTABLE: u32 = 0x0002_0000;
const SF_APPEND: u32 = 0x0004_0000;
const SF_RESTRICTED: u32 = 0x0008_0000;
const SF_NOUNLINK: u32 = 0x0010_0000;
const CLEAN_BUILD_VARIABLES: &[&str] = &[
    "ARCHFLAGS",
    "BASHOPTS",
    "BASH_ENV",
    "CC",
    "CFLAGS",
    "CDPATH",
    "CMAKE_GENERATOR",
    "CMAKE_GENERATOR_INSTANCE",
    "CMAKE_BUILD_PARALLEL_LEVEL",
    "CMAKE_C_COMPILER",
    "CMAKE_CXX_COMPILER",
    "CMAKE_OSX_ARCHITECTURES",
    "CMAKE_OSX_DEPLOYMENT_TARGET",
    "CMAKE_OSX_SYSROOT",
    "CMAKE_PREFIX_PATH",
    "CMAKE_TOOLCHAIN_FILE",
    "CPATH",
    "CPLUS_INCLUDE_PATH",
    "CXX",
    "CXXFLAGS",
    "CURL_HOME",
    "DYLD_FALLBACK_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "ENV",
    "GREP_OPTIONS",
    "LDFLAGS",
    "LIBRARY_PATH",
    "MACOSX_DEPLOYMENT_TARGET",
    "MAKEFLAGS",
    "OBJC_INCLUDE_PATH",
    "PKG_CONFIG_PATH",
    "SDKROOT",
    "SHELLOPTS",
    "UNZIP",
    "UNZIPOPT",
    "ZIPINFO",
    "ZIPINFOOPT",
];

#[derive(Debug, Clone)]
pub struct BuildEnvironment {
    pub path: OsString,
    pub developer_dir: PathBuf,
    pub cmake: PathBuf,
    pub ninja: PathBuf,
    home_dir: PathBuf,
    temporary_dir: PathBuf,
    proxy_environment: Vec<(OsString, OsString)>,
}

impl BuildEnvironment {
    pub fn configure(&self, command: &mut tokio::process::Command) {
        command
            .env("PATH", &self.path)
            .env("DEVELOPER_DIR", &self.developer_dir)
            .env("HOME", &self.home_dir)
            .env("TMPDIR", &self.temporary_dir)
            .env("XDG_CONFIG_HOME", self.home_dir.join(".config"));
        for variable in CLEAN_BUILD_VARIABLES {
            command.env_remove(variable);
        }
        for (variable, value) in &self.proxy_environment {
            command.env(variable, value);
        }
        for (variable, _) in std::env::vars_os() {
            if variable.to_string_lossy().starts_with("BASH_FUNC_")
                || variable.to_string_lossy().starts_with("GIT_CONFIG_")
            {
                command.env_remove(variable);
            }
        }
    }
}

#[derive(Debug, Clone)]
struct ToolProbe {
    path: Option<PathBuf>,
    detail: String,
}

#[derive(Debug, Clone)]
struct DeveloperToolsProbe {
    developer_dir: Option<PathBuf>,
    xcode_version: Option<[u64; 3]>,
    sdk_path: Option<PathBuf>,
    sdk_version: Option<[u64; 3]>,
    compiler_target: Option<String>,
    detail: String,
}

#[derive(Debug, Clone)]
struct ProxyProbe {
    compatible: bool,
    detail: String,
    environment: Vec<(OsString, OsString)>,
}

struct WorkingToolchain {
    environment: BuildEnvironment,
    cmake: ToolProbe,
    ninja: ToolProbe,
    developer_tools: DeveloperToolsProbe,
    detail: String,
}

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

    async fn preflight(
        &self,
        paths: &InstallerPaths,
        context: &PreflightContext,
    ) -> AppResult<PreflightReport> {
        let paths = paths.clone();
        let context = context.clone();
        tauri::async_runtime::spawn_blocking(move || run_preflight(&paths, &context))
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

fn discover(paths: &InstallerPaths, managed: &ManagedState) -> AppResult<Vec<InstallationInfo>> {
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
        if is_internal_installer_bundle(paths, &candidate) {
            continue;
        }
        if !candidate.exists() || !is_aseprite_bundle(&candidate) {
            continue;
        }
        let normalized = std::fs::canonicalize(&candidate).unwrap_or(candidate.clone());
        if !seen.insert(normalized.clone()) {
            continue;
        }
        let record = managed
            .installations
            .iter()
            .find(|record| paths_equal(Path::new(&record.path), &normalized));
        let managed_record = record.filter(|record| {
            record
                .bundle_fingerprint
                .as_deref()
                .is_some_and(|expected| {
                    bundle_fingerprint(&normalized)
                        .map(|actual| hex::encode(actual).eq_ignore_ascii_case(expected))
                        .unwrap_or(false)
                })
        });
        let channel = if managed_record.is_some() {
            InstallationChannel::Managed
        } else {
            infer_channel(&normalized)
        };
        let bundle_version = bundle_string(&normalized, "CFBundleShortVersionString");
        let version = managed_record
            .and_then(|record| record.source_version.clone())
            .or(bundle_version)
            .or_else(|| managed_record.map(|record| record.tag.clone()));
        let visible_path = managed_record
            .map(|record| PathBuf::from(&record.path))
            .unwrap_or_else(|| normalized.clone());
        let path_string = visible_path.to_string_lossy().into_owned();
        let writable = matches!(
            channel,
            InstallationChannel::Managed | InstallationChannel::Manual
        ) && visible_path
            .parent()
            .is_some_and(|parent| probe_directory_mutation(parent).is_ok());
        let manageable = matches!(
            channel,
            InstallationChannel::Managed | InstallationChannel::Manual
        ) && writable;
        let id = managed_record
            .map(|record| record.id.clone())
            .unwrap_or_else(|| installation_id(&path_string));
        let backup = managed_record
            .and_then(|record| record.backup_path.as_ref())
            .map(PathBuf::from)
            .filter(|path| {
                managed_record.is_some_and(|record| {
                    *path
                        == paths
                            .backups_dir
                            .join(format!("{}-previous.app", record.id))
                        && record
                            .backup_bundle_fingerprint
                            .as_deref()
                            .is_some_and(|expected| {
                                bundle_fingerprint(path)
                                    .map(|actual| {
                                        hex::encode(actual).eq_ignore_ascii_case(expected)
                                    })
                                    .unwrap_or(false)
                            })
                })
            });
        results.push(InstallationInfo {
            id,
            path: path_string,
            version,
            version_exact: managed_record
                .map(|record| record.version_exact)
                .unwrap_or(false),
            architecture: bundle_architecture(&normalized),
            channel,
            manageable,
            writable,
            has_backup: backup.is_some(),
            installed_at: managed_record.map(|record| record.installed_at.clone()),
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

fn is_internal_installer_bundle(paths: &InstallerPaths, candidate: &Path) -> bool {
    if candidate.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        [
            ".aseprite-installer-",
            ".aseprite-previous-",
            ".aseprite-current-",
            ".aseprite-restore-",
            ".aseprite-uninstall-",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
    }) {
        return true;
    }
    let normalized = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    [&paths.data_dir, &paths.cache_dir].iter().any(|root| {
        let normalized_root = std::fs::canonicalize(root).unwrap_or_else(|_| (*root).clone());
        normalized.starts_with(normalized_root)
    })
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

pub(crate) fn bundle_architecture(path: &Path) -> Option<String> {
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

pub(crate) fn bundle_fingerprint(bundle: &Path) -> AppResult<[u8; 32]> {
    let mut entries = WalkDir::new(bundle)
        .follow_links(false)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            InstallerError::with_detail(
                "targetFingerprint",
                "An Aseprite installation could not be fingerprinted safely.",
                error.to_string(),
            )
        })?;
    entries.sort_by(|left, right| left.path().cmp(right.path()));

    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(bundle).map_err(|error| {
            InstallerError::with_detail(
                "targetFingerprint",
                "An Aseprite installation contains an invalid path.",
                error.to_string(),
            )
        })?;
        let metadata = std::fs::symlink_metadata(path)?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_dir() {
            b'd'
        } else if file_type.is_file() {
            b'f'
        } else if file_type.is_symlink() {
            b'l'
        } else {
            b'o'
        };
        hasher.update([kind]);
        hasher.update(relative.as_os_str().as_bytes());
        hasher.update([0]);
        hasher.update(metadata.mode().to_le_bytes());

        if file_type.is_file() {
            let mut file = std::fs::File::open(path)?;
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
        } else if file_type.is_symlink() {
            hasher.update(std::fs::read_link(path)?.as_os_str().as_bytes());
        }
        hasher.update([0xff]);
    }
    Ok(hasher.finalize().into())
}

pub(crate) fn target_aseprite_running(target: &Path) -> Result<bool, String> {
    let Some(executable_name) = bundle_string(target, "CFBundleExecutable") else {
        return Ok(false);
    };
    let expected = target.join("Contents/MacOS").join(&executable_name);
    let expected = std::fs::canonicalize(&expected).unwrap_or(expected);
    let output = Command::new("/usr/bin/pgrep")
        .args(["-x", &executable_name])
        .output()
        .map_err(|error| format!("Could not inspect running Aseprite processes: {error}"))?;
    if !output.status.success() {
        return match output.status.code() {
            Some(1) => Ok(false),
            _ => Err(format!(
                "pgrep could not inspect running {executable_name} processes: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        };
    }

    for pid in String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<i32>().ok())
    {
        let path = process_path(pid).ok_or_else(|| {
            format!(
                "A running {executable_name} process ({pid}) could not be identified safely. Quit Aseprite before continuing."
            )
        })?;
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if path == expected {
            return Ok(true);
        }
    }
    Ok(false)
}

fn process_path(pid: i32) -> Option<PathBuf> {
    const PROC_PIDPATHINFO_MAXSIZE: usize = 4_096;
    unsafe extern "C" {
        fn proc_pidpath(pid: i32, buffer: *mut std::os::raw::c_void, buffer_size: u32) -> i32;
    }
    let mut buffer = [0_u8; PROC_PIDPATHINFO_MAXSIZE];
    // SAFETY: the writable buffer is valid for the declared size. proc_pidpath
    // returns a NUL-terminated path length or a non-positive error result.
    let length = unsafe {
        proc_pidpath(
            pid,
            buffer.as_mut_ptr().cast(),
            PROC_PIDPATHINFO_MAXSIZE as u32,
        )
    };
    if length <= 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(length as usize);
    Some(PathBuf::from(OsStr::from_bytes(&buffer[..end])))
}

fn infer_channel(path: &Path) -> InstallationChannel {
    if path.join("Contents/_MASReceipt/receipt").is_file() {
        return InstallationChannel::PackageManager;
    }
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

fn run_preflight(paths: &InstallerPaths, context: &PreflightContext) -> AppResult<PreflightReport> {
    let effective_user = run_command(Path::new("/usr/bin/id"), &["-u"], None, None);
    let non_elevated = effective_user
        .as_deref()
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .is_some_and(|user_id| user_id != 0);
    let os_version = run_command(
        Path::new("/usr/bin/sw_vers"),
        &["-productVersion"],
        None,
        None,
    )
    .unwrap_or_else(|_| "Unknown".into());
    let supported_os = parse_numeric_version(&os_version)
        .is_some_and(|version| version_at_least(version, [15, 2, 0]));
    let architecture_probe = run_command(Path::new("/usr/bin/uname"), &["-m"], None, None);
    let architecture_probe_detail = architecture_probe.as_ref().err().cloned();
    let architecture = architecture_probe.unwrap_or_else(|_| std::env::consts::ARCH.to_owned());
    let translated = process_is_translated(&architecture);
    let supported_architecture = architecture_probe_detail.is_none()
        && matches!(architecture.as_str(), "arm64" | "aarch64" | "x86_64");

    let mut developer_tools = probe_developer_tools();
    let mut cmake = probe_cmake(context.minimum_cmake_version);
    let mut ninja = probe_tool("ninja", None);
    let curl = probe_build_curl();
    let unzip = probe_build_unzip();
    let proxy = probe_system_proxy();
    let skia_route = if curl.path.is_some() && supported_architecture {
        probe_skia_download_route(&proxy, &architecture)
    } else {
        Err(
            "Resolve the Skia curl and architecture checks before testing the download route."
                .into(),
        )
    };
    let homebrew = if non_elevated {
        probe_homebrew()
    } else {
        ToolProbe {
            path: None,
            detail: "Skipped while the installer is running as root.".into(),
        }
    };
    let build_path = probe_build_path_compatibility(&paths.builds_dir);
    let case_sensitive_build = if non_elevated {
        probe_case_insensitive_build_volume(&paths.builds_dir)
    } else {
        Err("Skipped while the installer is running as root.".into())
    };
    let workspace = if non_elevated {
        probe_workspace(paths, !context.operation_lock_held)
    } else {
        Err("Skipped while the installer is running as root.".into())
    };
    let destination = if non_elevated {
        probe_install_destination(&context.target)
    } else {
        Err("Skipped while the installer is running as root.".into())
    };
    let target_state = probe_target_replacement(&context.target);
    let target_closed = probe_target_closed(&context.target);
    let free_space = available_space(&paths.cache_dir);
    let free_bytes = free_space.as_ref().copied().unwrap_or(0);

    let toolchain = if supported_architecture
        && developer_tools.developer_dir.is_some()
        && developer_tools.sdk_path.is_some()
        && cmake.path.is_some()
        && ninja.path.is_some()
        && build_path.is_ok()
        && case_sensitive_build.is_ok()
        && workspace.is_ok()
    {
        let tools_dir = paths.cache_dir.join(format!(
            ".aseprite-installer-preflight-tools-{}",
            Uuid::new_v4()
        ));
        let result = match select_working_toolchain(
            &paths.builds_dir,
            &tools_dir,
            context.minimum_cmake_version,
            &architecture,
            &proxy,
        ) {
            Ok(selected) => {
                cmake = selected.cmake;
                ninja = selected.ninja;
                developer_tools = selected.developer_tools;
                Ok(selected.detail)
            }
            Err(detail) => Err(augment_toolchain_failure(detail, &developer_tools)),
        };
        let _ = std::fs::remove_dir_all(&tools_dir);
        result
    } else {
        Err("Resolve the blocking tool and storage checks first.".into())
    };

    let baseline_ok = developer_tools.xcode_version == Some([16, 3, 0])
        && developer_tools.sdk_version == Some([15, 4, 0]);
    let sdk_ok = developer_tools.sdk_path.is_some() && developer_tools.sdk_version.is_some();
    let compiler_architecture_ok = developer_tools
        .compiler_target
        .as_deref()
        .is_some_and(|target| compiler_target_matches_architecture(target, &architecture));
    let xcode_ok = developer_tools.developer_dir.is_some() && compiler_architecture_ok;

    let prerequisites = vec![
        Prerequisite {
            id: "nonElevated".into(),
            label: "Normal user session".into(),
            ok: non_elevated,
            required: true,
            detail: effective_user
                .as_ref()
                .map(|user_id| format!("Effective user ID: {}", user_id.trim()))
                .unwrap_or_else(|error| error.clone()),
            remediation: (!non_elevated).then(|| {
                "Quit this elevated copy and reopen Aseprite Installer normally from Finder. Do not use sudo; no Mac restart is required."
                    .into()
            }),
        },
        Prerequisite {
            id: "macos".into(),
            label: "macOS".into(),
            ok: supported_os,
            required: true,
            detail: os_version.clone(),
            remediation: (!supported_os).then(|| {
                "This installer build requires macOS 15.2 or newer. Install the update and restart only if Software Update asks you to."
                    .into()
            }),
        },
        Prerequisite {
            id: "architecture".into(),
            label: "Build architecture".into(),
            ok: supported_architecture,
            required: true,
            detail: architecture_probe_detail
                .clone()
                .unwrap_or_else(|| architecture.clone()),
            remediation: (!supported_architecture).then(|| {
                "Use an arm64 or x86_64 installer build. The official build script has precompiled macOS Skia packages only for those architectures."
                    .into()
            }),
        },
        Prerequisite {
            id: "translation".into(),
            label: "Native execution".into(),
            ok: !translated,
            required: false,
            detail: if translated {
                format!("{architecture} through Rosetta on Apple silicon")
            } else {
                "Native process".into()
            },
            remediation: translated.then(|| {
                "A matching x86_64 compiler and the functional build test can still produce a working Intel build. Use the native arm64 installer to produce a native Apple silicon app."
                    .into()
            }),
        },
        Prerequisite {
            id: "xcode".into(),
            label: "Apple developer tools".into(),
            ok: xcode_ok,
            required: true,
            detail: developer_tools.detail.clone(),
            remediation: (!xcode_ok).then(|| {
                "Install Xcode or matching Apple Command Line Tools, finish any license/component prompts, and ensure the compiler target matches this installer architecture. Then check again; no restart is required."
                    .into()
            }),
        },
        Prerequisite {
            id: "sdk".into(),
            label: "macOS SDK".into(),
            ok: sdk_ok,
            required: true,
            detail: match (&developer_tools.sdk_version, &developer_tools.sdk_path) {
                (Some(version), Some(path)) => {
                    format!("{} · {}", format_version(*version), path.display())
                }
                _ => "No usable macOS SDK was exposed by xcrun.".into(),
            },
            remediation: (!sdk_ok).then(|| {
                "Finish Xcode setup or install matching Command Line Tools, then check again."
                    .into()
            }),
        },
        Prerequisite {
            id: "baseline".into(),
            label: "Aseprite documented baseline".into(),
            ok: baseline_ok,
            required: false,
            detail: format!(
                "Xcode {} · SDK {} (documented: Xcode 16.3 · SDK 15.4)",
                developer_tools
                    .xcode_version
                    .map(format_version)
                    .unwrap_or_else(|| "Command Line Tools".into()),
                developer_tools
                    .sdk_version
                    .map(format_version)
                    .unwrap_or_else(|| "unknown".into())
            ),
            remediation: (!baseline_ok).then(|| {
                "Aseprite documents Xcode 16.3 with SDK 15.4 as its tested baseline, while noting that other versions can work. A successful functional toolchain test is allowed to continue."
                    .into()
            }),
        },
        Prerequisite {
            id: "cmake".into(),
            label: "CMake".into(),
            ok: cmake.path.is_some(),
            required: true,
            detail: cmake.detail.clone(),
            remediation: cmake.path.is_none().then(|| {
                format!(
                    "Install CMake {} or newer for the selected source release, then check again.",
                    format_version(context.minimum_cmake_version)
                )
            }),
        },
        Prerequisite {
            id: "ninja".into(),
            label: "Ninja".into(),
            ok: ninja.path.is_some(),
            required: true,
            detail: ninja.detail.clone(),
            remediation: ninja.path.is_none().then(|| {
                "Install Ninja from its official release or Homebrew, then check again.".into()
            }),
        },
        Prerequisite {
            id: "curl".into(),
            label: "Skia download client".into(),
            ok: curl.path.is_some(),
            required: true,
            detail: curl.detail.clone(),
            remediation: curl.path.is_none().then(|| {
                "Restore the /usr/bin/curl supplied by macOS. The official build.sh uses it to download the matching precompiled Skia package; do not bypass TLS verification."
                    .into()
            }),
        },
        Prerequisite {
            id: "unzip".into(),
            label: "Skia archive extractor".into(),
            ok: unzip.path.is_some(),
            required: true,
            detail: unzip.detail.clone(),
            remediation: unzip.path.is_none().then(|| {
                "Restore the /usr/bin/unzip supplied by macOS, then check again. The official build.sh uses it for the Skia archive."
                    .into()
            }),
        },
        Prerequisite {
            id: "skiaProxy".into(),
            label: "Skia HTTPS/CDN route".into(),
            ok: skia_route.is_ok(),
            required: true,
            detail: skia_route.clone().unwrap_or_else(|error| error),
            remediation: skia_route.as_ref().err().map(|_| {
                "The official build.sh must reach the Skia release asset through command-line curl. Fix the reported HTTPS proxy/PAC/SOCKS, authentication, corporate CA, or GitHub/CDN allow-list issue, then check again. No Mac restart is required."
                    .into()
            }),
        },
        Prerequisite {
            id: "buildPath".into(),
            label: "Official build path".into(),
            ok: build_path.is_ok(),
            required: true,
            detail: build_path.clone().unwrap_or_else(|error| error),
            remediation: build_path.as_ref().err().map(|_| {
                "Use an installer cache path without spaces, tabs, or line breaks. The official Aseprite build.sh contains unquoted path expansions and cannot build safely from this location."
                    .into()
            }),
        },
        Prerequisite {
            id: "caseSensitiveBuild".into(),
            label: "Case-insensitive build volume".into(),
            ok: case_sensitive_build.is_ok(),
            required: true,
            detail: case_sensitive_build
                .clone()
                .unwrap_or_else(|error| error),
            remediation: case_sensitive_build.as_ref().err().map(|_| {
                "Use an installer cache located on a case-insensitive macOS volume. Aseprite’s current build creates Aseprite.app resources and an aseprite.app executable path that split on case-sensitive volumes."
                    .into()
            }),
        },
        Prerequisite {
            id: "workspace".into(),
            label: "Installer storage permissions".into(),
            ok: workspace.is_ok(),
            required: true,
            detail: workspace
                .as_ref()
                .map(|_| "Each installer folder passed the operations it actually uses: durable state/archive writes, executable and bundle-metadata build output, collision-safe archive/backup moves, atomic backup swaps, deletion, and both lock files.".into())
                .unwrap_or_else(|error| error.clone()),
            remediation: workspace.as_ref().err().map(|_| {
                "Restore write access to the installer data and cache folders; do not run the installer with sudo."
                    .into()
            }),
        },
        Prerequisite {
            id: "destination".into(),
            label: "Installation destination".into(),
            ok: destination.is_ok(),
            required: true,
            detail: destination.clone().unwrap_or_else(|error| error),
            remediation: destination.as_ref().err().map(|_| {
                "Choose or use a user-writable destination. A standard account can install in ~/Applications without administrator access."
                    .into()
            }),
        },
        Prerequisite {
            id: "targetState".into(),
            label: "Existing app replaceability".into(),
            ok: target_state.is_ok(),
            required: true,
            detail: target_state.clone().unwrap_or_else(|error| error),
            remediation: target_state.as_ref().err().map(|_| {
                "Remove Locked/append flags from the reported item, or ask an administrator/IT to remove system protection. Do not disable SIP or run the installer with sudo."
                    .into()
            }),
        },
        Prerequisite {
            id: "asepriteClosed".into(),
            label: "Selected Aseprite app is closed".into(),
            ok: target_closed.is_ok(),
            required: true,
            detail: target_closed.clone().unwrap_or_else(|error| error),
            remediation: target_closed.as_ref().err().map(|_| {
                "Quit the selected Aseprite copy, then check again. No Mac restart is required."
                    .into()
            }),
        },
        Prerequisite {
            id: "disk".into(),
            label: "Build space safety budget".into(),
            ok: free_space.is_ok() && free_bytes >= BUILD_SAFETY_BUDGET_BYTES,
            required: false,
            detail: if free_space.is_ok() {
                format!(
                    "{:.1} GB available in {}",
                    free_bytes as f64 / 1024_f64.powi(3),
                    paths.cache_dir.display()
                )
            } else {
                format!("Could not inspect {}", paths.cache_dir.display())
            },
            remediation: (free_space.is_err() || free_bytes < BUILD_SAFETY_BUDGET_BYTES)
                .then(|| "Free the installer’s recommended 6 GB safety budget on the cache volume. This is a conservative warning, not an upstream Aseprite minimum; exact capacity is rechecked before each download, install, backup, and restore mutation.".into()),
        },
        Prerequisite {
            id: "toolchain".into(),
            label: "C++17 build test".into(),
            ok: toolchain.is_ok(),
            required: true,
            detail: toolchain.clone().unwrap_or_else(|error| error),
            remediation: toolchain.as_ref().err().map(|_| {
                "Finish the Apple toolchain setup and verify CMake/Ninja, then run this check again. A reboot is not required."
                    .into()
            }),
        },
    ];
    let ready = prerequisites.iter().all(|item| !item.required || item.ok);
    Ok(PreflightReport {
        ready,
        architecture,
        os_version,
        free_bytes,
        minimum_free_bytes: BUILD_SAFETY_BUDGET_BYTES,
        homebrew_available: homebrew.path.is_some(),
        prerequisites,
    })
}

pub fn prepare_build_environment(
    minimum_cmake_version: [u64; 3],
    tools_dir: &Path,
) -> AppResult<BuildEnvironment> {
    let curl = probe_build_curl();
    let unzip = probe_build_unzip();
    if curl.path.is_none() || unzip.path.is_none() {
        return Err(InstallerError::with_detail(
            "buildEnvironment",
            "The system Skia download/extraction tools are no longer usable.",
            format!("curl: {}. unzip: {}.", curl.detail, unzip.detail),
        ));
    }
    let architecture =
        run_command(Path::new("/usr/bin/uname"), &["-m"], None, None).map_err(|detail| {
            InstallerError::with_detail(
                "buildEnvironment",
                "The runtime build architecture could not be inspected.",
                detail,
            )
        })?;
    let proxy = probe_system_proxy();
    probe_skia_download_route(&proxy, &architecture).map_err(|detail| {
        InstallerError::with_detail(
            "skiaNetworkChanged",
            "The command-line HTTPS route required for Skia no longer works.",
            detail,
        )
    })?;
    let build_root = tools_dir.parent().unwrap_or(tools_dir);
    select_working_toolchain(
        build_root,
        tools_dir,
        minimum_cmake_version,
        &architecture,
        &proxy,
    )
    .map(|selection| selection.environment)
    .map_err(|detail| {
        InstallerError::with_detail(
            "buildEnvironment",
            "No available CMake, Ninja, and Apple-toolchain combination passed the functional build test.",
            detail,
        )
    })
}

pub fn validate_build_environment(
    paths: &InstallerPaths,
    environment: &BuildEnvironment,
) -> AppResult<()> {
    probe_build_path_compatibility(&paths.builds_dir).map_err(|detail| {
        InstallerError::with_detail(
            "buildPath",
            "The official build path is no longer compatible.",
            detail,
        )
    })?;
    probe_case_insensitive_build_volume(&paths.builds_dir).map_err(|detail| {
        InstallerError::with_detail(
            "caseSensitiveBuild",
            "The build volume is no longer compatible with Aseprite’s bundle output.",
            detail,
        )
    })?;
    // begin_operation() already owns .operation.lock at this point. Reopen and
    // relock the same file through the workspace probe would conflict with our
    // own interprocess guard on filesystems where flock is open-description
    // scoped, so only revalidate the registry lock here.
    probe_workspace(paths, false).map_err(|detail| {
        InstallerError::with_detail(
            "workspace",
            "The installer workspace is no longer writable.",
            detail,
        )
    })?;
    let architecture =
        run_command(Path::new("/usr/bin/uname"), &["-m"], None, None).map_err(|detail| {
            InstallerError::with_detail(
                "buildEnvironment",
                "The runtime build architecture could not be inspected.",
                detail,
            )
        })?;
    smoke_test_toolchain(&paths.builds_dir, environment, &architecture)
        .map(|_| ())
        .map_err(|detail| {
            InstallerError::with_detail(
                "toolchainChanged",
                "The build toolchain changed or stopped working after preflight.",
                detail,
            )
        })
}

pub fn usable_homebrew_path() -> Result<PathBuf, String> {
    let probe = probe_homebrew();
    probe.path.ok_or(probe.detail)
}

fn probe_homebrew() -> ToolProbe {
    let brew = probe_tool("brew", None);
    let Some(path) = brew.path.as_ref() else {
        return brew;
    };
    let mut checked = Vec::new();
    for argument in ["--prefix", "--cellar", "--cache", "--repository"] {
        let directory = match run_command(path, &[argument], None, Some(OsStr::new(SYSTEM_PATH))) {
            Ok(output) => PathBuf::from(output),
            Err(error) => {
                return ToolProbe {
                    path: None,
                    detail: format!(
                        "{} {argument} failed: {error}. Use Homebrew’s supported repair/doctor workflow; do not run brew with sudo.",
                        path.display()
                    ),
                };
            }
        };
        if let Err(error) = probe_directory_capabilities(&directory, HOMEBREW_DIRECTORY_PROBE) {
            return ToolProbe {
                path: None,
                detail: format!(
                    "Homebrew cannot safely write in {}: {error}. Repair this Homebrew installation without sudo, or install CMake and Ninja manually.",
                    directory.display()
                ),
            };
        }
        checked.push(directory);
    }
    ToolProbe {
        path: Some(path.clone()),
        detail: format!(
            "{} · write probes passed for {}",
            brew.detail,
            checked
                .iter()
                .map(|directory| directory.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn probe_cmake(minimum_version: [u64; 3]) -> ToolProbe {
    probe_tool("cmake", Some(minimum_version))
}

fn probe_build_curl() -> ToolProbe {
    let path = PathBuf::from("/usr/bin/curl");
    if !is_executable_file(&path) {
        return ToolProbe {
            path: None,
            detail: "/usr/bin/curl is missing or not executable.".into(),
        };
    }
    match run_command(&path, &["--help", "all"], None, Some(OsStr::new(SYSTEM_PATH))) {
        Ok(output)
            if [
                "--disable",
                "--fail",
                "--connect-timeout",
                "--retry-all-errors",
                "--ssl-revoke-best-effort",
            ]
            .iter()
            .all(|option| output.contains(option)) =>
        {
            ToolProbe {
                path: Some(path.clone()),
                detail: format!("Required HTTPS/retry options available · {}", path.display()),
            }
        }
        Ok(_) => ToolProbe {
            path: None,
            detail: "/usr/bin/curl does not support the options required by the official build and installer retry wrapper.".into(),
        },
        Err(error) => ToolProbe {
            path: None,
            detail: format!("{}: {error}", path.display()),
        },
    }
}

fn probe_build_unzip() -> ToolProbe {
    let path = PathBuf::from("/usr/bin/unzip");
    if !is_executable_file(&path) {
        return ToolProbe {
            path: None,
            detail: "/usr/bin/unzip is missing or not executable.".into(),
        };
    }
    match run_command(&path, &["-v"], None, Some(OsStr::new(SYSTEM_PATH))) {
        Ok(output) => ToolProbe {
            path: Some(path.clone()),
            detail: format!("{} · {}", first_line(&output), path.display()),
        },
        Err(error) => ToolProbe {
            path: None,
            detail: format!("{}: {error}", path.display()),
        },
    }
}

fn probe_system_proxy() -> ProxyProbe {
    let (explicit_https, mut issues) = explicit_proxy_variable(&["https_proxy", "HTTPS_PROXY"]);
    let (explicit_all, all_issues) = explicit_proxy_variable(&["all_proxy", "ALL_PROXY"]);
    issues.extend(all_issues);
    let (explicit_http, http_issues) = explicit_proxy_variable(&["http_proxy", "HTTP_PROXY"]);
    let mut notes = http_issues;
    for variable in ["CURL_CA_BUNDLE", "SSL_CERT_FILE"] {
        let Some(path) = std::env::var_os(variable).filter(|value| !value.is_empty()) else {
            continue;
        };
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                notes.push(format!("explicit {variable} trust file"));
            }
            Ok(_) => issues.push(format!("{variable} does not name a regular file")),
            Err(error) => issues.push(format!("{variable} cannot be read: {error}")),
        }
    }
    let output = match run_command(
        Path::new("/usr/sbin/scutil"),
        &["--proxy"],
        None,
        Some(OsStr::new(SYSTEM_PATH)),
    ) {
        Ok(output) => output,
        Err(error) => {
            let compatible =
                (explicit_https.is_some() || explicit_all.is_some()) && issues.is_empty();
            return ProxyProbe {
                compatible,
                detail: if compatible {
                    format!(
                        "An explicit curl proxy environment is available; macOS proxy settings could not be inspected: {error}. The actual GitHub/CDN request remains the runtime network test."
                    )
                } else {
                    format!(
                        "macOS proxy settings could not be inspected for the build.sh curl route: {error}{}",
                        format_proxy_issues(&issues)
                    )
                },
                environment: Vec::new(),
            };
        }
    };
    let values = output
        .lines()
        .filter_map(|line| line.trim().split_once(" : "))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect::<BTreeMap<_, _>>();
    let enabled = |key: &str| values.get(key).is_some_and(|value| value == "1");
    let mut environment = Vec::new();
    let mut routes = Vec::new();

    let https_enabled = enabled("HTTPSEnable");
    let static_https = if https_enabled {
        proxy_url(&values, "HTTPSProxy", "HTTPSPort")
    } else {
        None
    };
    if let Some(variable) = explicit_https {
        routes.push(format!("explicit {variable}"));
    } else if let Some(proxy) = static_https.as_ref() {
        for variable in ["HTTPS_PROXY", "https_proxy"] {
            environment.push((OsString::from(variable), OsString::from(proxy)));
        }
        routes.push(format!("macOS static HTTPS proxy {proxy}"));
    } else if let Some(variable) = explicit_all {
        routes.push(format!("explicit {variable}"));
    }

    let static_http = if enabled("HTTPEnable") {
        proxy_url(&values, "HTTPProxy", "HTTPPort")
    } else {
        None
    };
    if let Some(variable) = explicit_http {
        routes.push(format!("explicit {variable}"));
    } else if let Some(proxy) = static_http.as_ref() {
        environment.push((OsString::from("http_proxy"), OsString::from(proxy)));
        routes.push(format!("macOS static HTTP proxy {proxy}"));
    }

    let automatic = enabled("ProxyAutoConfigEnable") || enabled("ProxyAutoDiscoveryEnable");
    let socks_enabled = enabled("SOCKSEnable");
    let static_socks = if socks_enabled {
        proxy_url_with_scheme(&values, "SOCKSProxy", "SOCKSPort", "socks5h")
    } else {
        None
    };
    if explicit_https.is_none() && explicit_all.is_none() && static_https.is_none() {
        if let Some(proxy) = static_socks.as_ref() {
            for variable in ["ALL_PROXY", "all_proxy"] {
                environment.push((OsString::from(variable), OsString::from(proxy)));
            }
            routes.push(format!("macOS static SOCKS proxy {proxy}"));
        }
    }
    let compatible = issues.is_empty()
        && (explicit_https.is_some()
            || explicit_all.is_some()
            || static_https.is_some()
            || static_socks.is_some()
            || (!automatic && !socks_enabled && !https_enabled));
    let detail = if !issues.is_empty() {
        format!(
            "The build.sh curl environment is inconsistent: {}.{}",
            issues.join("; "),
            if routes.is_empty() {
                String::new()
            } else {
                format!(" Configured routes: {}.", routes.join(" · "))
            }
        )
    } else if !routes.is_empty() {
        format!(
            "{}; settings are refreshed again when the official build starts. Route syntax is valid; the actual GitHub/CDN request remains the runtime network test.{}",
            routes.join(" · "),
            format_proxy_notes(&notes)
        )
    } else if https_enabled {
        "The enabled static macOS HTTPS proxy has a missing or invalid host/port and cannot be passed safely to build.sh curl.".into()
    } else if automatic {
        "macOS uses PAC/WPAD without an explicit/static HTTPS proxy; API networking and build.sh curl may not follow the same route.".into()
    } else if socks_enabled {
        "The enabled macOS SOCKS proxy has a missing or invalid host/port and cannot be passed safely to build.sh curl.".into()
    } else {
        format!(
            "No HTTPS proxy is enabled; build.sh curl will use the direct route. The actual GitHub/CDN request remains the runtime network test.{}",
            format_proxy_notes(&notes)
        )
    };
    ProxyProbe {
        compatible,
        detail,
        environment,
    }
}

fn proxy_url(values: &BTreeMap<String, String>, host_key: &str, port_key: &str) -> Option<String> {
    proxy_url_with_scheme(values, host_key, port_key, "http")
}

fn proxy_url_with_scheme(
    values: &BTreeMap<String, String>,
    host_key: &str,
    port_key: &str,
    scheme: &str,
) -> Option<String> {
    let host = values.get(host_key)?.trim();
    let port = values
        .get(port_key)?
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)?;
    if host.is_empty()
        || host
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'@'))
    {
        return None;
    }
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let proxy = format!("{scheme}://{host}:{port}");
    valid_proxy_environment_value(&proxy).then_some(proxy)
}

fn explicit_proxy_variable(names: &[&'static str]) -> (Option<&'static str>, Vec<String>) {
    let mut valid = Vec::new();
    let mut issues = Vec::new();
    let mut distinct_values = BTreeSet::new();
    for &name in names {
        let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) else {
            continue;
        };
        let shown = value.to_string_lossy();
        if valid_proxy_environment_value(&shown) {
            distinct_values.insert(shown.into_owned());
            valid.push(name);
        } else {
            issues.push(format!("{name} is malformed"));
        }
    }
    if distinct_values.len() > 1 {
        issues.push(format!(
            "{} contain conflicting proxy values",
            names.join(" and ")
        ));
    }
    (valid.into_iter().next(), issues)
}

fn valid_proxy_environment_value(value: &str) -> bool {
    if value.is_empty() || value.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return false;
    }
    let candidate = if value.contains("://") {
        value.to_owned()
    } else {
        format!("http://{value}")
    };
    Url::parse(&candidate).is_ok_and(|url| {
        matches!(
            url.scheme(),
            "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h"
        ) && url.host_str().is_some()
            && matches!(url.path(), "" | "/")
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn format_proxy_issues(issues: &[String]) -> String {
    if issues.is_empty() {
        String::new()
    } else {
        format!(" Environment issues: {}.", issues.join("; "))
    }
}

fn format_proxy_notes(notes: &[String]) -> String {
    if notes.is_empty() {
        String::new()
    } else {
        format!(" Additional environment: {}.", notes.join("; "))
    }
}

fn probe_skia_download_route(proxy: &ProxyProbe, architecture: &str) -> Result<String, String> {
    let cpu = match architecture {
        "arm64" | "aarch64" => "arm64",
        "x86_64" => "x64",
        other => return Err(format!("No official macOS Skia route exists for {other}.")),
    };
    let url = format!(
        "https://github.com/aseprite/skia/releases/download/{SKIA_ROUTE_PROBE_TAG}/Skia-macOS-Release-{cpu}.zip"
    );
    let result = run_command_with_timeout_environment(
        Path::new("/usr/bin/curl"),
        &[
            "--disable",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--range",
            "0-0",
            "--output",
            "/dev/null",
            "--max-filesize",
            "1048576",
            "--write-out",
            "%{http_code}\n%{size_download}\n%{content_type}\n%{url_effective}",
            "--connect-timeout",
            "20",
            "--max-time",
            "30",
            "--retry",
            "1",
            "--retry-all-errors",
            "--ssl-revoke-best-effort",
            &url,
        ],
        None,
        Some(OsStr::new(SYSTEM_PATH)),
        &proxy.environment,
        Duration::from_secs(40),
    );
    match result {
        Ok(output) if valid_skia_route_response(&output) => Ok(format!(
            "Command-line curl completed a one-byte HTTPS GET through GitHub’s Skia release redirect/CDN for {cpu}. {}{}",
            proxy.detail,
            if proxy.compatible {
                ""
            } else {
                " The live request succeeded despite the static proxy warning."
            }
        )),
        Ok(_) => Err(format!(
            "Command-line curl returned an unexpected response instead of one byte from the official Skia release CDN. {}",
            proxy.detail
        )),
        Err(error) => Err(format!(
            "Command-line curl could not reach the official Skia release asset and CDN: {error}. {}",
            proxy.detail
        )),
    }
}

fn valid_skia_route_response(output: &str) -> bool {
    let mut lines = output.lines();
    let status = lines.next();
    let size = lines.next().and_then(|value| value.parse::<f64>().ok());
    let content_type = lines.next();
    let effective_url = lines.next().and_then(|value| Url::parse(value).ok());
    status == Some("206")
        && size.is_some_and(|size| (size - 1.0).abs() < f64::EPSILON)
        && content_type == Some("application/octet-stream")
        && effective_url
            .as_ref()
            .and_then(Url::host_str)
            .is_some_and(|host| host == "release-assets.githubusercontent.com")
}

fn probe_tool(name: &str, minimum_version: Option<[u64; 3]>) -> ToolProbe {
    let (successes, failures) = tool_candidates(name, minimum_version);
    successes.into_iter().next().unwrap_or_else(|| ToolProbe {
        path: None,
        detail: if failures.is_empty() {
            format!("{name} was not found in PATH or a standard command-line location.")
        } else {
            failures.join("; ")
        },
    })
}

fn tool_candidates(name: &str, minimum_version: Option<[u64; 3]>) -> (Vec<ToolProbe>, Vec<String>) {
    let mut failures = Vec::new();
    let mut successes = Vec::new();
    for (index, path) in executable_candidates(name).into_iter().enumerate() {
        if !path.exists() {
            continue;
        }
        if !is_executable_file(&path) {
            failures.push(format!("{} is not executable", path.display()));
            continue;
        }
        match run_command(&path, &["--version"], None, None) {
            Ok(output) => {
                let version = parse_numeric_version(&output);
                if let Some(minimum_version) = minimum_version {
                    if !version.is_some_and(|version| version_at_least(version, minimum_version)) {
                        failures.push(format!(
                            "{} reports {} (need {} or newer)",
                            path.display(),
                            version
                                .map(format_version)
                                .unwrap_or_else(|| "an unknown version".into()),
                            format_version(minimum_version)
                        ));
                        continue;
                    }
                }
                let shown_version = version
                    .map(format_version)
                    .unwrap_or_else(|| first_line(&output));
                successes.push((
                    version.unwrap_or([0, 0, 0]),
                    index,
                    path.clone(),
                    format!("{shown_version} · {}", path.display()),
                ));
            }
            Err(error) => failures.push(format!("{}: {error}", path.display())),
        }
    }
    successes.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let successes = successes
        .into_iter()
        .map(|(_, _, path, detail)| ToolProbe {
            path: Some(path),
            detail,
        })
        .collect();
    (successes, failures)
}

fn executable_candidates(name: &str) -> Vec<PathBuf> {
    let mut directories = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .filter(|directory| directory.is_absolute())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    directories.extend(if std::env::consts::ARCH == "x86_64" {
        vec![
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/opt/homebrew/bin"),
        ]
    } else {
        vec![
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]
    });
    directories.push(PathBuf::from("/opt/local/bin"));
    if let Some(home) = dirs::home_dir() {
        directories.push(home.join(".local/bin"));
    }
    if name == "cmake" {
        directories.push(PathBuf::from("/Applications/CMake.app/Contents/bin"));
        if let Some(home) = dirs::home_dir() {
            directories.push(home.join("Applications/CMake.app/Contents/bin"));
        }
    }
    directories.push(PathBuf::from("/usr/bin"));
    directories.push(PathBuf::from("/bin"));

    let mut seen = BTreeSet::new();
    directories
        .into_iter()
        .map(|directory| directory.join(name))
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn probe_developer_tools() -> DeveloperToolsProbe {
    let (successes, failures) = developer_tools_candidates();
    successes
        .into_iter()
        .next()
        .unwrap_or_else(|| DeveloperToolsProbe {
            developer_dir: None,
            xcode_version: None,
            sdk_path: None,
            sdk_version: None,
            compiler_target: None,
            detail: if failures.is_empty() {
                "No active Apple developer directory was found.".into()
            } else {
                failures.join("; ")
            },
        })
}

fn developer_tools_candidates() -> (Vec<DeveloperToolsProbe>, Vec<String>) {
    let mut failures = Vec::new();
    let mut successes = Vec::new();
    for developer_dir in developer_dir_candidates() {
        if !developer_dir.is_dir() {
            failures.push(format!("{} is missing", developer_dir.display()));
            continue;
        }
        let sdk_path = run_command(
            Path::new("/usr/bin/xcrun"),
            &["--sdk", "macosx", "--show-sdk-path"],
            Some(&developer_dir),
            Some(OsStr::new(SYSTEM_PATH)),
        );
        let sdk_version = run_command(
            Path::new("/usr/bin/xcrun"),
            &["--sdk", "macosx", "--show-sdk-version"],
            Some(&developer_dir),
            Some(OsStr::new(SYSTEM_PATH)),
        );
        let compiler_target = run_command(
            Path::new("/usr/bin/xcrun"),
            &["--sdk", "macosx", "clang++", "-dumpmachine"],
            Some(&developer_dir),
            Some(OsStr::new(SYSTEM_PATH)),
        );
        match (sdk_path, sdk_version, compiler_target) {
            (Ok(sdk_path), Ok(sdk_version), Ok(compiler_target))
                if Path::new(&sdk_path).is_dir() =>
            {
                let xcode_version = run_command(
                    Path::new("/usr/bin/xcodebuild"),
                    &["-version"],
                    Some(&developer_dir),
                    Some(OsStr::new(SYSTEM_PATH)),
                )
                .ok()
                .and_then(|output| parse_numeric_version(&output));
                let parsed_sdk_version = parse_numeric_version(&sdk_version);
                let tool_label = xcode_version
                    .map(|version| format!("Xcode {}", format_version(version)))
                    .unwrap_or_else(|| "Apple Command Line Tools".into());
                successes.push(DeveloperToolsProbe {
                    developer_dir: Some(developer_dir.clone()),
                    xcode_version,
                    sdk_path: Some(PathBuf::from(sdk_path)),
                    sdk_version: parsed_sdk_version,
                    compiler_target: Some(compiler_target.clone()),
                    detail: format!(
                        "{tool_label} · {} · compiler target {compiler_target}",
                        developer_dir.display()
                    ),
                });
            }
            (sdk_path, sdk_version, compiler_target) => {
                let invalid_sdk_path = sdk_path.as_ref().ok().and_then(|path| {
                    (!Path::new(path).is_dir())
                        .then(|| format!("xcrun returned a missing SDK path: {path}"))
                });
                let detail = [
                    sdk_path.err(),
                    sdk_version.err(),
                    compiler_target.err(),
                    invalid_sdk_path,
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("; ");
                failures.push(format!("{}: {detail}", developer_dir.display()));
            }
        }
    }
    (successes, failures)
}

fn developer_dir_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let active = run_command(Path::new("/usr/bin/xcode-select"), &["-p"], None, None)
        .ok()
        .map(PathBuf::from);
    if let Some(active) = active
        .as_ref()
        .filter(|path| !path.ends_with("CommandLineTools"))
    {
        candidates.push(active.clone());
    }
    candidates.push(PathBuf::from("/Applications/Xcode.app/Contents/Developer"));
    let mut alternatives = Vec::new();
    if let Ok(applications) = std::fs::read_dir("/Applications") {
        for entry in applications.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("Xcode") && name.ends_with(".app") {
                alternatives.push(entry.path().join("Contents/Developer"));
            }
        }
    }
    alternatives.sort();
    candidates.extend(alternatives);
    if let Some(active) = active {
        candidates.push(active);
    }
    let mut seen = BTreeSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

fn select_working_toolchain(
    build_root: &Path,
    tools_dir: &Path,
    minimum_cmake_version: [u64; 3],
    expected_architecture: &str,
    proxy: &ProxyProbe,
) -> Result<WorkingToolchain, String> {
    let (cmake_candidates, cmake_failures) = tool_candidates("cmake", Some(minimum_cmake_version));
    if cmake_candidates.is_empty() {
        return Err(candidate_failure_detail("CMake", &cmake_failures));
    }
    let (ninja_candidates, ninja_failures) = tool_candidates("ninja", None);
    if ninja_candidates.is_empty() {
        return Err(candidate_failure_detail("Ninja", &ninja_failures));
    }
    let (developer_candidates, developer_failures) = developer_tools_candidates();
    if developer_candidates.is_empty() {
        return Err(candidate_failure_detail(
            "Apple developer tools",
            &developer_failures,
        ));
    }

    let mut failures = Vec::new();
    let mut attempts = 0_usize;
    for developer_tools in &developer_candidates {
        if !developer_tools
            .compiler_target
            .as_deref()
            .is_some_and(|target| {
                compiler_target_matches_architecture(target, expected_architecture)
            })
        {
            failures.push(format!(
                "{} does not target {expected_architecture}",
                developer_tools.detail
            ));
            continue;
        }
        for cmake in &cmake_candidates {
            for ninja in &ninja_candidates {
                attempts += 1;
                let environment =
                    match create_build_environment(tools_dir, cmake, ninja, developer_tools, proxy)
                    {
                        Ok(environment) => environment,
                        Err(error) => {
                            failures.push(format!(
                                "{} + {} + {}: {error}",
                                cmake.detail, ninja.detail, developer_tools.detail
                            ));
                            continue;
                        }
                    };
                match smoke_test_toolchain(build_root, &environment, expected_architecture) {
                    Ok(smoke_detail) => {
                        return Ok(WorkingToolchain {
                            environment,
                            cmake: cmake.clone(),
                            ninja: ninja.clone(),
                            developer_tools: developer_tools.clone(),
                            detail: format!(
                                "{smoke_detail} Selected {} · {} · {}",
                                cmake.detail, ninja.detail, developer_tools.detail
                            ),
                        });
                    }
                    Err(error) => failures.push(format!(
                        "{} + {} + {}: {error}",
                        cmake.detail, ninja.detail, developer_tools.detail
                    )),
                }
            }
        }
    }

    let shown = failures.iter().take(12).cloned().collect::<Vec<_>>();
    let omitted = failures.len().saturating_sub(shown.len());
    let omitted_detail = (omitted > 0).then(|| format!("; {omitted} more failures omitted"));
    Err(format!(
        "No available toolchain combination passed after {attempts} functional attempt(s): {}{}",
        shown.join("; "),
        omitted_detail.unwrap_or_default()
    ))
}

fn candidate_failure_detail(name: &str, failures: &[String]) -> String {
    if failures.is_empty() {
        format!("{name} was not found in PATH or a standard installation location.")
    } else {
        format!(
            "No usable {name} candidate was found: {}",
            failures.join("; ")
        )
    }
}

fn create_build_environment(
    tools_dir: &Path,
    cmake: &ToolProbe,
    ninja: &ToolProbe,
    developer_tools: &DeveloperToolsProbe,
    proxy: &ProxyProbe,
) -> Result<BuildEnvironment, String> {
    let cmake = cmake.path.as_ref().ok_or_else(|| cmake.detail.clone())?;
    let ninja = ninja.path.as_ref().ok_or_else(|| ninja.detail.clone())?;
    let developer_dir = developer_tools
        .developer_dir
        .as_ref()
        .ok_or_else(|| developer_tools.detail.clone())?;
    std::fs::create_dir_all(tools_dir)
        .map_err(|error| format!("Could not create {}: {error}", tools_dir.display()))?;
    create_tool_link(cmake, &tools_dir.join("cmake"))?;
    create_tool_link(ninja, &tools_dir.join("ninja"))?;
    create_curl_wrapper(&tools_dir.join("curl"))?;
    let mut path = tools_dir.as_os_str().to_os_string();
    path.push(":");
    path.push(SYSTEM_PATH);
    let environment_root = tools_dir.parent().unwrap_or(tools_dir);
    let home_dir = environment_root.join("home");
    let temporary_dir = environment_root.join("tmp");
    std::fs::create_dir_all(home_dir.join(".config"))
        .map_err(|error| format!("Could not create {}: {error}", home_dir.display()))?;
    std::fs::create_dir_all(&temporary_dir)
        .map_err(|error| format!("Could not create {}: {error}", temporary_dir.display()))?;
    let proxy_environment = proxy.environment.clone();
    Ok(BuildEnvironment {
        path,
        developer_dir: developer_dir.clone(),
        cmake: tools_dir.join("cmake"),
        ninja: tools_dir.join("ninja"),
        home_dir,
        temporary_dir,
        proxy_environment,
    })
}

fn create_curl_wrapper(destination: &Path) -> Result<(), String> {
    std::fs::write(
        destination,
        b"#!/bin/sh\nexec /usr/bin/curl --disable --fail --connect-timeout 20 --retry 3 --retry-all-errors --speed-limit 1 --speed-time 30 \"$@\"\n",
    )
    .map_err(|error| format!("Could not create {}: {error}", destination.display()))?;
    let mut permissions = std::fs::metadata(destination)
        .map_err(|error| format!("Could not inspect {}: {error}", destination.display()))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(destination, permissions).map_err(|error| {
        format!(
            "Could not make {} executable: {error}",
            destination.display()
        )
    })
}

fn create_tool_link(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.symlink_metadata().is_ok() {
        std::fs::remove_file(destination)
            .map_err(|error| format!("Could not replace {}: {error}", destination.display()))?;
    }
    symlink(source, destination).map_err(|error| {
        format!(
            "Could not link {} to {}: {error}",
            destination.display(),
            source.display()
        )
    })
}

fn augment_toolchain_failure(detail: String, developer_tools: &DeveloperToolsProbe) -> String {
    let Some(developer_dir) = developer_tools.developer_dir.as_deref() else {
        return detail;
    };
    if developer_tools.xcode_version.is_none() {
        return detail;
    }
    match run_command(
        Path::new("/usr/bin/xcodebuild"),
        &["-checkFirstLaunchStatus"],
        Some(developer_dir),
        Some(OsStr::new(SYSTEM_PATH)),
    ) {
        Ok(_) => format!("{detail} Xcode reports that first-launch setup is complete."),
        Err(diagnostic) => {
            format!("{detail} Xcode first-launch/license diagnostic: {diagnostic}")
        }
    }
}

fn smoke_test_toolchain(
    build_root: &Path,
    environment: &BuildEnvironment,
    expected_architecture: &str,
) -> Result<String, String> {
    let root = build_root.join(format!(".aseprite-installer-preflight-{}", Uuid::new_v4()));
    let cleanup = CleanupDirectory(root.clone());
    let source = root.join("source");
    let build = root.join("build");
    std::fs::create_dir_all(&source)
        .map_err(|error| format!("Could not create the build probe: {error}"))?;
    std::fs::write(
        source.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.16)\nproject(aseprite_installer_probe CXX)\nset(CMAKE_CXX_STANDARD 17)\nset(CMAKE_CXX_STANDARD_REQUIRED ON)\nadd_executable(aseprite-installer-probe main.cpp)\n",
    )
    .map_err(|error| format!("Could not write the CMake probe: {error}"))?;
    std::fs::write(
        source.join("main.cpp"),
        "#include <optional>\nint main() { std::optional<int> value = 0; return *value; }\n",
    )
    .map_err(|error| format!("Could not write the C++17 probe: {error}"))?;

    let source_arg = source.to_string_lossy().into_owned();
    let build_arg = build.to_string_lossy().into_owned();
    let ninja_arg = format!("-DCMAKE_MAKE_PROGRAM={}", environment.ninja.display());
    run_command_with_timeout(
        &environment.cmake,
        &[
            "-S",
            &source_arg,
            "-B",
            &build_arg,
            "-G",
            "Ninja",
            "-DCMAKE_BUILD_TYPE=Release",
            "-DCMAKE_OSX_DEPLOYMENT_TARGET=11.0",
            &ninja_arg,
        ],
        Some(&environment.developer_dir),
        Some(&environment.path),
        Duration::from_secs(120),
    )
    .map_err(|error| format!("CMake configure probe failed: {error}"))?;
    run_command_with_timeout(
        &environment.cmake,
        &[
            "--build",
            &build_arg,
            "--target",
            "aseprite-installer-probe",
        ],
        Some(&environment.developer_dir),
        Some(&environment.path),
        Duration::from_secs(120),
    )
    .map_err(|error| format!("C++17 compile/link probe failed: {error}"))?;
    let compiled_executable = build.join("aseprite-installer-probe");
    let probe_bundle = root.join("AsepriteInstallerProbe.app");
    let probe_macos = probe_bundle.join("Contents/MacOS");
    let probe_resources = probe_bundle.join("Contents/Resources");
    std::fs::create_dir_all(&probe_macos)
        .and_then(|_| std::fs::create_dir_all(&probe_resources))
        .map_err(|error| format!("Could not create the code-signing bundle probe: {error}"))?;
    let probe_executable = probe_macos.join("aseprite-installer-probe");
    std::fs::copy(&compiled_executable, &probe_executable)
        .map_err(|error| format!("Could not stage the bundle probe executable: {error}"))?;
    std::fs::set_permissions(
        &probe_executable,
        std::fs::metadata(&compiled_executable)
            .map_err(|error| format!("Could not inspect the compiled probe: {error}"))?
            .permissions(),
    )
    .map_err(|error| format!("Could not preserve the probe executable mode: {error}"))?;
    std::fs::write(
        probe_bundle.join("Contents/Info.plist"),
        br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>aseprite-installer-probe</string>
<key>CFBundleIdentifier</key><string>com.fmhun.aseprite-installer.probe</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>1.0</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>
"#,
    )
    .and_then(|_| std::fs::write(probe_resources.join("sealed-resource.txt"), b"verified"))
    .map_err(|error| format!("Could not write the code-signing bundle probe: {error}"))?;
    let probe_executable_arg = probe_executable.to_string_lossy().into_owned();
    let probe_bundle_arg = probe_bundle.to_string_lossy().into_owned();
    let architectures = run_command(
        Path::new("/usr/bin/lipo"),
        &["-archs", &probe_executable_arg],
        Some(&environment.developer_dir),
        Some(&environment.path),
    )
    .map_err(|error| format!("The compiled probe architecture could not be read: {error}"))?;
    if !binary_architectures_match(&architectures, expected_architecture) {
        return Err(format!(
            "The functional probe produced {architectures}, but the installer/build script architecture is {expected_architecture}. Use matching native CMake, Ninja, and Apple developer tools."
        ));
    }
    run_command(
        Path::new("/usr/bin/codesign"),
        &["--force", "--deep", "--sign", "-", &probe_bundle_arg],
        Some(&environment.developer_dir),
        Some(&environment.path),
    )
    .map_err(|error| format!("Ad-hoc code-signing probe failed: {error}"))?;
    run_command(
        Path::new("/usr/bin/codesign"),
        &["--verify", "--deep", "--strict", &probe_bundle_arg],
        Some(&environment.developer_dir),
        Some(&environment.path),
    )
    .map_err(|error| format!("Code-signature verification probe failed: {error}"))?;
    run_command(
        &probe_executable,
        &[],
        Some(&environment.developer_dir),
        Some(&environment.path),
    )
    .map_err(|error| format!("The compiled probe could not run: {error}"))?;
    drop(cleanup);
    Ok("CMake configured, compiled, linked, packaged, deep ad-hoc signed, verified, and ran a C++17 app-bundle executable with Ninja.".into())
}

fn binary_architectures_match(architectures: &str, expected: &str) -> bool {
    let expected = match expected {
        "aarch64" => "arm64",
        value => value,
    };
    architectures.split_whitespace().any(|architecture| {
        architecture == expected || (expected == "arm64" && architecture == "arm64e")
    })
}

fn probe_build_path_compatibility(builds_dir: &Path) -> Result<String, String> {
    if builds_dir
        .as_os_str()
        .as_bytes()
        .iter()
        .any(u8::is_ascii_whitespace)
    {
        return Err(format!(
            "The official build.sh cannot safely use a build path containing whitespace: {}",
            builds_dir.display()
        ));
    }
    Ok(format!(
        "No whitespace detected in {}",
        builds_dir.display()
    ))
}

fn probe_case_insensitive_build_volume(builds_dir: &Path) -> Result<String, String> {
    std::fs::create_dir_all(builds_dir)
        .map_err(|error| format!("Could not create {}: {error}", builds_dir.display()))?;
    let probe_root = builds_dir.join(format!(".aseprite-installer-case-probe-{}", Uuid::new_v4()));
    std::fs::create_dir(&probe_root).map_err(|error| {
        format!(
            "Could not create the case-sensitivity probe in {}: {error}",
            builds_dir.display()
        )
    })?;
    let cleanup = CleanupDirectory(probe_root.clone());
    let result = (|| {
        let canonical_spelling = probe_root.join("Aseprite.app");
        let alternate_spelling = probe_root.join("aseprite.app");
        std::fs::create_dir(&canonical_spelling).map_err(|error| {
            format!(
                "Could not create the case-sensitivity probe in {}: {error}",
                builds_dir.display()
            )
        })?;
        let canonical_metadata =
            std::fs::symlink_metadata(&canonical_spelling).map_err(|error| {
                format!(
                    "Could not inspect the case-sensitivity probe in {}: {error}",
                    builds_dir.display()
                )
            })?;

        match std::fs::symlink_metadata(&alternate_spelling) {
            Ok(alternate_metadata)
                if alternate_metadata.dev() == canonical_metadata.dev()
                    && alternate_metadata.ino() == canonical_metadata.ino() =>
            {
                Ok(format!(
                    "Aseprite.app and aseprite.app resolve to the same entry in {}",
                    builds_dir.display()
                ))
            }
            Ok(_) => Err(format!(
                "The build volume distinguishes Aseprite.app from aseprite.app in {}",
                builds_dir.display()
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(format!(
                "The build volume is case-sensitive: Aseprite.app and aseprite.app are distinct in {}",
                builds_dir.display()
            )),
            Err(error) => Err(format!(
                "Could not verify case sensitivity in {}: {error}",
                builds_dir.display()
            )),
        }
    })();
    let cleanup_error = std::fs::remove_dir_all(&probe_root).err();
    drop(cleanup);
    if let Some(error) = cleanup_error {
        return Err(format!(
            "Could not remove the case-sensitivity probe from {}: {error}",
            builds_dir.display()
        ));
    }
    result
}

fn probe_workspace(paths: &InstallerPaths, probe_operation_lock: bool) -> Result<(), String> {
    for (directory, capabilities) in [
        (&paths.data_dir, REGISTRY_DIRECTORY_PROBE),
        (&paths.cache_dir, BASIC_DIRECTORY_PROBE),
        (&paths.archives_dir, ARCHIVE_DIRECTORY_PROBE),
        (&paths.builds_dir, BUILD_DIRECTORY_PROBE),
        (&paths.logs_dir, BASIC_DIRECTORY_PROBE),
        (&paths.backups_dir, BACKUP_DIRECTORY_PROBE),
    ] {
        probe_directory_capabilities(directory, capabilities)?;
    }
    probe_registry_storage(paths, probe_operation_lock)?;
    Ok(())
}

fn probe_registry_storage(
    paths: &InstallerPaths,
    probe_operation_lock: bool,
) -> Result<(), String> {
    if let Ok(metadata) = std::fs::symlink_metadata(&paths.registry_file) {
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(format!(
                "{} is not a regular registry file.",
                paths.registry_file.display()
            ));
        }
        std::fs::File::open(&paths.registry_file).map_err(|error| {
            format!(
                "Cannot read the managed-installation registry {}: {error}",
                paths.registry_file.display()
            )
        })?;
        let flags = MacOsMetadataExt::st_flags(&metadata);
        if flags
            & (UF_IMMUTABLE | UF_APPEND | SF_IMMUTABLE | SF_APPEND | SF_RESTRICTED | SF_NOUNLINK)
            != 0
        {
            return Err(format!(
                "{} has flags that prevent an atomic registry update (0x{flags:08x}).",
                paths.registry_file.display()
            ));
        }
    }

    let lock_files = if probe_operation_lock {
        &[".managed-state.lock", ".operation.lock"][..]
    } else {
        &[".managed-state.lock"][..]
    };
    for file_name in lock_files {
        let lock_path = paths.data_dir.join(file_name);
        if lock_path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(format!("{} is a symbolic link.", lock_path.display()));
        }
        let lock = open_lock_file(&lock_path)
            .map_err(|error| format!("Cannot open {} safely: {error}", lock_path.display()))?;
        lock.try_lock_exclusive().map_err(|error| {
            format!(
                "Cannot lock {}. Another installer process may be active, or the file has incompatible ownership/permissions: {error}",
                lock_path.display()
            )
        })?;
        FileExt::unlock(&lock)
            .map_err(|error| format!("Cannot unlock {}: {error}", lock_path.display()))?;
    }
    Ok(())
}

fn probe_install_destination(target: &Path) -> Result<String, String> {
    let parent = target
        .parent()
        .ok_or_else(|| "The installation target has no parent directory.".to_owned())?;
    probe_directory_mutation(parent)?;
    let resolved = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    let free_bytes = available_space(&resolved).map_err(|error| {
        format!(
            "Could not inspect free space in {}: {error}",
            resolved.display()
        )
    })?;
    let free = format!(
        " · {:.1} GB available",
        free_bytes as f64 / 1024_f64.powi(3)
    );
    Ok(format!(
        "Create/write/fsync, real execution, symlink, extended-attribute, collision-safe rename, atomic directory-swap, and delete probes passed in {}{free}",
        resolved.display()
    ))
}

fn probe_target_replacement(target: &Path) -> Result<String, String> {
    let metadata = match std::fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok("No existing item needs to be replaced.".into());
        }
        Err(error) => {
            return Err(format!("Could not inspect {}: {error}", target.display()));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "{} is a symbolic link; linked installation targets are not replaced.",
            target.display()
        ));
    }
    if !metadata.is_dir() || !is_aseprite_bundle(target) {
        return Err(format!(
            "{} is not a valid Aseprite application bundle.",
            target.display()
        ));
    }
    const PROTECTED_FLAGS: u32 =
        UF_IMMUTABLE | UF_APPEND | SF_IMMUTABLE | SF_APPEND | SF_RESTRICTED | SF_NOUNLINK;
    for entry in WalkDir::new(target).follow_links(false) {
        let entry =
            entry.map_err(|error| format!("Could not inspect {}: {error}", target.display()))?;
        let metadata = std::fs::symlink_metadata(entry.path())
            .map_err(|error| format!("Could not inspect {}: {error}", entry.path().display()))?;
        let flags = MacOsMetadataExt::st_flags(&metadata);
        if flags & PROTECTED_FLAGS != 0 {
            return Err(format!(
                "{} has immutable/append/no-unlink/restricted flags (0x{flags:08x}).",
                entry.path().display()
            ));
        }
    }
    Ok(format!(
        "{} is a valid Aseprite bundle with no blocking file flags.",
        target.display()
    ))
}

fn probe_target_closed(target: &Path) -> Result<String, String> {
    match target_aseprite_running(target) {
        Ok(false) => Ok(if target.exists() {
            "The selected Aseprite executable is not running.".into()
        } else {
            "No existing target process needs to be closed.".into()
        }),
        Ok(true) => Err(format!(
            "The Aseprite executable inside {} is currently running.",
            target.display()
        )),
        Err(error) => Err(error),
    }
}

#[derive(Clone, Copy)]
struct DirectoryProbeCapabilities {
    file_sync: bool,
    directory_sync: bool,
    execute: bool,
    symlink: bool,
    extended_attribute: bool,
    rename_exclusive: bool,
    rename_swap: bool,
}

const BASIC_DIRECTORY_PROBE: DirectoryProbeCapabilities = DirectoryProbeCapabilities {
    file_sync: false,
    directory_sync: false,
    execute: false,
    symlink: false,
    extended_attribute: false,
    rename_exclusive: false,
    rename_swap: false,
};
const REGISTRY_DIRECTORY_PROBE: DirectoryProbeCapabilities = DirectoryProbeCapabilities {
    file_sync: true,
    ..BASIC_DIRECTORY_PROBE
};
const ARCHIVE_DIRECTORY_PROBE: DirectoryProbeCapabilities = DirectoryProbeCapabilities {
    file_sync: true,
    directory_sync: true,
    rename_exclusive: true,
    ..BASIC_DIRECTORY_PROBE
};
const BUILD_DIRECTORY_PROBE: DirectoryProbeCapabilities = DirectoryProbeCapabilities {
    execute: true,
    symlink: true,
    extended_attribute: true,
    ..BASIC_DIRECTORY_PROBE
};
const BACKUP_DIRECTORY_PROBE: DirectoryProbeCapabilities = DirectoryProbeCapabilities {
    symlink: true,
    extended_attribute: true,
    rename_exclusive: true,
    rename_swap: true,
    ..BASIC_DIRECTORY_PROBE
};
const DESTINATION_DIRECTORY_PROBE: DirectoryProbeCapabilities = DirectoryProbeCapabilities {
    execute: true,
    symlink: true,
    extended_attribute: true,
    rename_exclusive: true,
    rename_swap: true,
    ..BASIC_DIRECTORY_PROBE
};
const HOMEBREW_DIRECTORY_PROBE: DirectoryProbeCapabilities = DirectoryProbeCapabilities {
    execute: true,
    symlink: true,
    ..BASIC_DIRECTORY_PROBE
};

pub(crate) fn probe_directory_mutation(directory: &Path) -> Result<(), String> {
    probe_directory_capabilities(directory, DESTINATION_DIRECTORY_PROBE)
}

fn probe_directory_capabilities(
    directory: &Path,
    capabilities: DirectoryProbeCapabilities,
) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
    let suffix = Uuid::new_v4();
    let original = directory.join(format!(".aseprite-installer-write-{suffix}"));
    let renamed = directory.join(format!(".aseprite-installer-rename-{suffix}"));
    let result = (|| {
        std::fs::create_dir(&original).map_err(|error| {
            format!(
                "Cannot create a directory in {}: {error}",
                directory.display()
            )
        })?;
        let payload = original.join("executable-probe");
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&payload)
            .map_err(|error| format!("Cannot write in {}: {error}", directory.display()))?;
        file.write_all(b"#!/bin/sh\nexit 0\n")
            .map_err(|error| format!("Cannot write in {}: {error}", directory.display()))?;
        if capabilities.file_sync {
            file.sync_all().map_err(|error| {
                format!("Cannot persist a file in {}: {error}", directory.display())
            })?;
        }
        drop(file);
        if capabilities.execute {
            let mut permissions = std::fs::metadata(&payload)
                .map_err(|error| format!("Cannot inspect {}: {error}", payload.display()))?
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&payload, permissions).map_err(|error| {
                format!(
                    "Cannot set executable permissions in {}: {error}",
                    directory.display()
                )
            })?;
            if std::fs::metadata(&payload)
                .map_err(|error| format!("Cannot inspect {}: {error}", payload.display()))?
                .permissions()
                .mode()
                & 0o111
                == 0
            {
                return Err(format!(
                    "{} does not preserve executable permissions.",
                    directory.display()
                ));
            }
            run_command(&payload, &[], None, Some(OsStr::new(SYSTEM_PATH))).map_err(|error| {
                format!(
                    "Cannot execute a newly created file in {}: {error}",
                    directory.display()
                )
            })?;
        }
        let payload_arg = payload.to_string_lossy().into_owned();
        if capabilities.symlink {
            symlink("executable-probe", original.join("executable-link")).map_err(|error| {
                format!(
                    "Cannot create symbolic links in {}: {error}",
                    directory.display()
                )
            })?;
        }
        if capabilities.extended_attribute {
            run_command(
                Path::new("/usr/bin/xattr"),
                &[
                    "-w",
                    "com.fmhun.aseprite-installer-probe",
                    "verified",
                    &payload_arg,
                ],
                None,
                Some(OsStr::new(SYSTEM_PATH)),
            )
            .map_err(|error| {
                format!(
                    "Cannot write extended attributes in {}: {error}",
                    directory.display()
                )
            })?;
            let attribute = run_command(
                Path::new("/usr/bin/xattr"),
                &["-p", "com.fmhun.aseprite-installer-probe", &payload_arg],
                None,
                Some(OsStr::new(SYSTEM_PATH)),
            )
            .map_err(|error| {
                format!(
                    "Cannot read extended attributes in {}: {error}",
                    directory.display()
                )
            })?;
            if attribute.trim() != "verified" {
                return Err(format!(
                    "{} did not preserve an extended attribute.",
                    directory.display()
                ));
            }
        }
        if capabilities.directory_sync {
            std::fs::File::open(&original)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    format!(
                        "Cannot persist a directory in {}: {error}",
                        directory.display()
                    )
                })?;
        }
        if capabilities.rename_exclusive {
            std::fs::create_dir(&renamed).map_err(|error| {
                format!(
                    "Cannot prepare a collision sentinel in {}: {error}",
                    directory.display()
                )
            })?;
            std::fs::write(renamed.join("occupied-marker"), b"occupied").map_err(|error| {
                format!(
                    "Cannot write a collision sentinel in {}: {error}",
                    directory.display()
                )
            })?;
            match rename_exclusive_probe(&original, &renamed) {
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(format!(
                        "Collision-safe rename returned the wrong error in {}: {error}",
                        directory.display()
                    ));
                }
                Ok(()) => {
                    return Err(format!(
                        "Collision-safe rename overwrote an occupied destination in {}.",
                        directory.display()
                    ));
                }
            }
            if !original.join("executable-probe").is_file()
                || !renamed.join("occupied-marker").is_file()
            {
                return Err(format!(
                    "{} did not preserve both sides of a refused collision-safe rename.",
                    directory.display()
                ));
            }
            std::fs::remove_dir_all(&renamed).map_err(|error| {
                format!(
                    "Cannot delete a collision sentinel in {}: {error}",
                    directory.display()
                )
            })?;
            rename_exclusive_probe(&original, &renamed).map_err(|error| {
                format!(
                    "Cannot perform a collision-safe rename in {}: {error}",
                    directory.display()
                )
            })?;
        }
        if capabilities.rename_swap {
            std::fs::create_dir(&original).map_err(|error| {
                format!(
                    "Cannot prepare an atomic-swap probe in {}: {error}",
                    directory.display()
                )
            })?;
            std::fs::write(original.join("swap-marker"), b"placeholder").map_err(|error| {
                format!(
                    "Cannot write an atomic-swap probe in {}: {error}",
                    directory.display()
                )
            })?;
            rename_swap_probe(&original, &renamed).map_err(|error| {
                format!(
                    "Cannot atomically swap application directories in {}: {error}",
                    directory.display()
                )
            })?;
            if !original.join("executable-probe").is_file()
                || !renamed.join("swap-marker").is_file()
            {
                return Err(format!(
                    "{} did not preserve both sides of an atomic directory swap.",
                    directory.display()
                ));
            }
        }
        for probe_path in [&original, &renamed] {
            if probe_path.symlink_metadata().is_ok() {
                std::fs::remove_dir_all(probe_path).map_err(|error| {
                    format!("Cannot delete in {}: {error}", directory.display())
                })?;
            }
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&original);
    let _ = std::fs::remove_dir_all(&renamed);
    result
}

fn rename_exclusive_probe(source: &Path, destination: &Path) -> std::io::Result<()> {
    rename_with_flags_probe(source, destination, 0x0000_0004)
}

fn rename_swap_probe(source: &Path, destination: &Path) -> std::io::Result<()> {
    rename_with_flags_probe(source, destination, 0x0000_0002)
}

fn rename_with_flags_probe(source: &Path, destination: &Path, flags: u32) -> std::io::Result<()> {
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
    // SAFETY: both pointers refer to valid NUL-terminated strings for the call.
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

fn process_is_translated(process_architecture: &str) -> bool {
    let translated_flag = run_command(
        Path::new("/usr/sbin/sysctl"),
        &["-in", "sysctl.proc_translated"],
        None,
        None,
    )
    .is_ok_and(|output| output.trim() == "1");
    let apple_silicon = run_command(
        Path::new("/usr/sbin/sysctl"),
        &["-in", "hw.optional.arm64"],
        None,
        None,
    )
    .is_ok_and(|output| output.trim() == "1");
    translated_flag || (apple_silicon && process_architecture == "x86_64")
}

fn compiler_target_matches_architecture(target: &str, architecture: &str) -> bool {
    match architecture {
        "arm64" | "aarch64" => target.starts_with("arm64-") || target.starts_with("aarch64-"),
        "x86_64" => target.starts_with("x86_64-"),
        _ => false,
    }
}

fn run_command(
    program: &Path,
    arguments: &[&str],
    developer_dir: Option<&Path>,
    path: Option<&OsStr>,
) -> Result<String, String> {
    run_command_with_timeout(
        program,
        arguments,
        developer_dir,
        path,
        Duration::from_secs(20),
    )
}

fn run_command_with_timeout(
    program: &Path,
    arguments: &[&str],
    developer_dir: Option<&Path>,
    path: Option<&OsStr>,
    timeout: Duration,
) -> Result<String, String> {
    run_command_with_timeout_environment(program, arguments, developer_dir, path, &[], timeout)
}

fn run_command_with_timeout_environment(
    program: &Path,
    arguments: &[&str],
    developer_dir: Option<&Path>,
    path: Option<&OsStr>,
    environment: &[(OsString, OsString)],
    timeout: Duration,
) -> Result<String, String> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(developer_dir) = developer_dir {
        command.env("DEVELOPER_DIR", developer_dir);
    }
    if let Some(path) = path {
        command.env("PATH", path);
    }
    for (variable, value) in environment {
        command.env(variable, value);
    }
    for variable in CLEAN_BUILD_VARIABLES {
        command.env_remove(variable);
    }
    for (variable, _) in std::env::vars_os() {
        if variable.to_string_lossy().starts_with("BASH_FUNC_") {
            command.env_remove(variable);
        }
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("{} could not start: {error}", program.display()))?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            kill_process_group(&mut child);
            return Err(format!("{} stdout was unavailable", program.display()));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            kill_process_group(&mut child);
            return Err(format!("{} stderr was unavailable", program.display()));
        }
    };
    let stdout_reader = spawn_bounded_output_reader(stdout);
    let stderr_reader = spawn_bounded_output_reader(stderr);
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                kill_process_group(&mut child);
                let _ = collect_bounded_output(stdout_reader, "stdout");
                let _ = collect_bounded_output(stderr_reader, "stderr");
                return Err(format!(
                    "{} timed out after {} seconds",
                    program.display(),
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                kill_process_group(&mut child);
                let _ = collect_bounded_output(stdout_reader, "stdout");
                let _ = collect_bounded_output(stderr_reader, "stderr");
                return Err(format!(
                    "{} could not be monitored: {error}",
                    program.display()
                ));
            }
        }
    };
    let stdout = collect_bounded_output(stdout_reader, "stdout")?;
    let stderr = collect_bounded_output(stderr_reader, "stderr")?;
    let stdout = String::from_utf8_lossy(&stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
    if status.success() {
        if !stdout.is_empty() {
            Ok(stdout)
        } else if !stderr.is_empty() {
            Ok(stderr)
        } else {
            Ok("Succeeded".into())
        }
    } else {
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        Err(if detail.is_empty() {
            format!("exited with {status}")
        } else {
            tail(&detail, 1_200)
        })
    }
}

fn spawn_bounded_output_reader<R>(reader: R) -> JoinHandle<std::io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || read_bounded_output(reader))
}

fn read_bounded_output<R>(mut reader: R) -> std::io::Result<Vec<u8>>
where
    R: Read,
{
    let mut output = Vec::with_capacity(COMMAND_OUTPUT_LIMIT_BYTES);
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        append_bounded_tail(&mut output, &chunk[..count]);
    }
    Ok(output)
}

fn append_bounded_tail(output: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.len() >= COMMAND_OUTPUT_LIMIT_BYTES {
        output.clear();
        output.extend_from_slice(&bytes[bytes.len() - COMMAND_OUTPUT_LIMIT_BYTES..]);
        return;
    }
    let overflow = output
        .len()
        .saturating_add(bytes.len())
        .saturating_sub(COMMAND_OUTPUT_LIMIT_BYTES);
    if overflow > 0 {
        output.drain(..overflow);
    }
    output.extend_from_slice(bytes);
}

fn collect_bounded_output(
    reader: JoinHandle<std::io::Result<Vec<u8>>>,
    stream: &str,
) -> Result<Vec<u8>, String> {
    reader
        .join()
        .map_err(|_| format!("The {stream} reader stopped unexpectedly."))?
        .map_err(|error| format!("The {stream} stream could not be read: {error}"))
}

fn kill_process_group(child: &mut Child) {
    let process_group = format!("-{}", child.id());
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &process_group])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

fn parse_numeric_version(value: &str) -> Option<[u64; 3]> {
    value
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .filter(|part| !part.is_empty() && part.contains('.'))
        .find_map(|part| {
            let mut numbers = part.split('.').take(3).map(str::parse::<u64>);
            let major = numbers.next()?.ok()?;
            let minor = numbers.next()?.ok()?;
            let patch = numbers.next().and_then(Result::ok).unwrap_or(0);
            Some([major, minor, patch])
        })
}

fn version_at_least(version: [u64; 3], minimum: [u64; 3]) -> bool {
    version >= minimum
}

fn format_version(version: [u64; 3]) -> String {
    if version[2] == 0 {
        format!("{}.{}", version[0], version[1])
    } else {
        format!("{}.{}.{}", version[0], version[1], version[2])
    }
}

fn first_line(value: &str) -> String {
    value.lines().next().unwrap_or(value).trim().to_owned()
}

fn tail(value: &str, maximum_chars: usize) -> String {
    let mut characters = value.chars().rev().take(maximum_chars).collect::<Vec<_>>();
    characters.reverse();
    characters.into_iter().collect()
}

struct CleanupDirectory(PathBuf);

impl Drop for CleanupDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

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

    #[test]
    fn parses_tool_and_operating_system_versions() {
        assert_eq!(
            parse_numeric_version("cmake version 3.20.6"),
            Some([3, 20, 6])
        );
        assert_eq!(
            parse_numeric_version("Xcode 16.3\nBuild version 16E140"),
            Some([16, 3, 0])
        );
        assert_eq!(parse_numeric_version("15.2"), Some([15, 2, 0]));
        assert_eq!(parse_numeric_version("not a version"), None);
        assert!(version_at_least([3, 20, 0], [3, 20, 0]));
        assert!(!version_at_least([3, 19, 9], [3, 20, 0]));
    }

    #[test]
    fn matches_apple_compiler_targets_to_the_build_architecture() {
        assert!(compiler_target_matches_architecture(
            "arm64-apple-macosx15.0.0",
            "arm64"
        ));
        assert!(compiler_target_matches_architecture(
            "x86_64-apple-darwin24.0.0",
            "x86_64"
        ));
        assert!(!compiler_target_matches_architecture(
            "arm64-apple-macosx15.0.0",
            "x86_64"
        ));
    }

    #[test]
    fn verifies_effective_directory_mutation_permissions() {
        let directory = tempfile::tempdir().unwrap();
        probe_directory_mutation(directory.path()).unwrap();
        assert!(std::fs::read_dir(directory.path())
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn validates_proxy_urls_without_exposing_credentials() {
        assert!(valid_proxy_environment_value("proxy.example:8443"));
        assert!(valid_proxy_environment_value(
            "socks5h://user:secret@proxy.example:1080"
        ));
        assert!(!valid_proxy_environment_value("http://proxy.example/path"));
        assert!(!valid_proxy_environment_value("http://proxy example:8080"));

        let mut values = BTreeMap::from([
            ("HTTPSProxy".into(), "2001:db8::1".into()),
            ("HTTPSPort".into(), "8443".into()),
        ]);
        assert_eq!(
            proxy_url(&values, "HTTPSProxy", "HTTPSPort").as_deref(),
            Some("http://[2001:db8::1]:8443")
        );
        values.insert("HTTPSPort".into(), "0".into());
        assert!(proxy_url(&values, "HTTPSProxy", "HTTPSPort").is_none());
        values.insert("HTTPSPort".into(), "8443".into());
        values.insert("HTTPSProxy".into(), "unsafe/path".into());
        assert!(proxy_url(&values, "HTTPSProxy", "HTTPSPort").is_none());
    }

    #[test]
    fn workspace_revalidation_does_not_relock_the_owned_operation_guard() {
        let directory = tempfile::tempdir().unwrap();
        let paths = InstallerPaths::new(
            directory.path().join("data"),
            directory.path().join("cache"),
        );
        paths.ensure().unwrap();
        let state = AppState::new(paths.clone()).unwrap();
        state.begin_operation().unwrap();

        assert!(probe_workspace(&paths, false).is_ok());
        assert!(probe_workspace(&paths, true).is_err());
    }

    #[test]
    fn separates_build_path_compatibility_from_storage_permissions() {
        assert!(probe_build_path_compatibility(Path::new("/tmp/aseprite-builds")).is_ok());
        let error = probe_build_path_compatibility(Path::new("/tmp/aseprite builds")).unwrap_err();
        assert!(error.contains("whitespace"));
    }

    #[test]
    fn detects_the_build_volume_case_behavior_without_leaving_probe_files() {
        let directory = tempfile::tempdir().unwrap();
        let direct_probe = directory.path().join("direct-probe");
        std::fs::create_dir(&direct_probe).unwrap();
        let canonical = direct_probe.join("Aseprite.app");
        let alternate = direct_probe.join("aseprite.app");
        std::fs::create_dir(&canonical).unwrap();
        let expected_case_insensitive = std::fs::symlink_metadata(&alternate)
            .ok()
            .zip(std::fs::symlink_metadata(&canonical).ok())
            .is_some_and(|(alternate, canonical)| {
                alternate.dev() == canonical.dev() && alternate.ino() == canonical.ino()
            });
        std::fs::remove_dir_all(&direct_probe).unwrap();

        let result = probe_case_insensitive_build_volume(directory.path());

        assert_eq!(result.is_ok(), expected_case_insensitive);
        assert!(std::fs::read_dir(directory.path())
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn drains_command_output_larger_than_an_os_pipe() {
        let output = run_command_with_timeout(
            Path::new("/bin/sh"),
            &["-c", "/usr/bin/yes aseprite | /usr/bin/head -c 262144"],
            None,
            Some(OsStr::new(SYSTEM_PATH)),
            Duration::from_secs(5),
        )
        .unwrap();

        assert!(output.contains("aseprite"));
        assert!(output.len() <= COMMAND_OUTPUT_LIMIT_BYTES);
    }

    #[test]
    fn timeout_kills_the_spawned_process_group() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("orphan-marker");
        let marker_argument = marker.to_string_lossy().into_owned();
        let result = run_command_with_timeout(
            Path::new("/bin/sh"),
            &[
                "-c",
                "(/bin/sleep 1; /usr/bin/touch \"$1\") & wait",
                "aseprite-timeout-probe",
                &marker_argument,
            ],
            None,
            Some(OsStr::new(SYSTEM_PATH)),
            Duration::from_millis(100),
        );

        assert!(result.is_err());
        std::thread::sleep(Duration::from_millis(1_200));
        assert!(!marker.exists());
    }

    #[test]
    #[ignore = "requires the live macOS developer toolchain, CMake, Ninja, codesign, and HTTPS"]
    fn live_preflight_configures_compiles_links_and_runs() {
        let directory = tempfile::tempdir().unwrap();
        let paths = InstallerPaths::new(
            directory.path().join("data"),
            directory.path().join("cache"),
        );
        let context = PreflightContext {
            target: directory.path().join("Applications/Aseprite.app"),
            minimum_cmake_version: [3, 20, 0],
            operation_lock_held: false,
        };

        let report = run_preflight(&paths, &context).unwrap();
        assert!(
            report.ready,
            "{}",
            report
                .prerequisites
                .iter()
                .filter(|requirement| requirement.required && !requirement.ok)
                .map(|requirement| format!("{}: {}", requirement.label, requirement.detail))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
}
