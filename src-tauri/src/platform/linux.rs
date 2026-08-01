use super::{PlatformAdapter, PreflightContext};
use crate::error::{AppResult, InstallerError};
use crate::models::{
    InstallationChannel, InstallationInfo, ManagedState, PreflightReport, Prerequisite,
};
use crate::portable_transaction::{IntegrationSnapshot, MAX_INTEGRATION_FILE_BYTES};
use crate::state::InstallerPaths;
use async_trait::async_trait;
use fs2::available_space;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CString, OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use uuid::Uuid;
use walkdir::WalkDir;

const BUILD_SAFETY_BUDGET_BYTES: u64 = 6 * 1024 * 1024 * 1024;
const COMMAND_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DISCOVERY_DESKTOP_ENTRY_BYTES: u64 = 128 * 1024;
const MAX_DISCOVERY_DESKTOP_ENTRIES: usize = 2_048;
const MINIMUM_CLANG_VERSION: [u64; 3] = [12, 0, 0];
const MINIMUM_CMAKE_VERSION: [u64; 3] = [3, 20, 0];
const MINIMUM_NINJA_VERSION: [u64; 3] = [1, 10, 0];
const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const DESKTOP_PREFIX: &str = "aseprite-installer-";
const FLATPAK_ASEPRITE_ID: &str = "org.aseprite.Aseprite";
const DANGEROUS_RUNTIME_VARIABLES: &[&str] = &[
    "BASH_ENV",
    "CDPATH",
    "ENV",
    "GCONV_PATH",
    "GTK_PATH",
    "LD_AUDIT",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "PERL5LIB",
    "PYTHONHOME",
    "PYTHONPATH",
    "RUBYLIB",
];

#[derive(Debug, Clone)]
pub struct BuildEnvironment {
    pub path: OsString,
    pub clang: PathBuf,
    pub clangxx: PathBuf,
    pub cmake: PathBuf,
    pub ninja: PathBuf,
    home_dir: PathBuf,
    temporary_dir: PathBuf,
}

impl BuildEnvironment {
    pub fn configure(&self, command: &mut tokio::process::Command) {
        command
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", &self.home_dir)
            .env("TMPDIR", &self.temporary_dir)
            .env("XDG_CONFIG_HOME", self.home_dir.join(".config"))
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .env("CC", &self.clang)
            .env("CXX", &self.clangxx)
            .env("CMAKE_GENERATOR", "Ninja")
            .env("CMAKE_MAKE_PROGRAM", &self.ninja);
    }

    pub fn configure_std(&self, command: &mut Command) {
        command
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", &self.home_dir)
            .env("TMPDIR", &self.temporary_dir)
            .env("XDG_CONFIG_HOME", self.home_dir.join(".config"))
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .env("CC", &self.clang)
            .env("CXX", &self.clangxx)
            .env("CMAKE_GENERATOR", "Ninja")
            .env("CMAKE_MAKE_PROGRAM", &self.ninja);
    }
}

#[derive(Debug, Clone)]
struct ToolProbe {
    path: Option<PathBuf>,
    version: Option<[u64; 3]>,
    detail: String,
}

#[derive(Debug, Clone)]
struct LinuxDistribution {
    id: String,
    id_like: String,
    pretty_name: String,
}

pub struct LinuxAdapter;

impl LinuxAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PlatformAdapter for LinuxAdapter {
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
                    "Aseprite installations could not be scanned on Linux.",
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
                    "The Linux build environment could not be checked.",
                    error.to_string(),
                )
            })?
    }

    fn default_target(&self) -> AppResult<PathBuf> {
        Ok(xdg_data_home()?.join("aseprite-installer/managed/Aseprite"))
    }
}

pub fn installation_id(path: &str) -> String {
    let digest = Sha256::digest(path.as_bytes());
    format!("aseprite-{}", &hex::encode(digest)[..16])
}

pub fn artifact_fingerprint(root: &Path) -> AppResult<[u8; 32]> {
    let root_metadata = std::fs::symlink_metadata(root).map_err(|error| {
        InstallerError::with_detail(
            "artifactFingerprint",
            "The Aseprite artifact could not be fingerprinted.",
            format!("{}: {error}", root.display()),
        )
    })?;
    if root_metadata.file_type().is_symlink()
        || (!root_metadata.file_type().is_dir() && !root_metadata.file_type().is_file())
    {
        return Err(InstallerError::with_detail(
            "artifactType",
            "The Aseprite artifact root must be a regular file or directory.",
            root.display().to_string(),
        ));
    }
    let root_snapshot = unix_snapshot(&root_metadata);

    let mut entries = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            InstallerError::with_detail(
                "artifactFingerprint",
                "The complete Aseprite artifact tree could not be inspected.",
                error.to_string(),
            )
        })?;
    entries.sort_by(|left, right| left.path().cmp(right.path()));

    let mut hasher = Sha256::new();
    hasher.update(b"aseprite-installer/linux-artifact-fingerprint/v2\0");
    let mut buffer = [0_u8; 128 * 1024];
    let mut manifest = BTreeMap::new();
    let mut directory_snapshots = Vec::new();
    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|error| {
            InstallerError::with_detail(
                "artifactPath",
                "The Aseprite artifact contains an invalid path.",
                error.to_string(),
            )
        })?;
        let metadata = std::fs::symlink_metadata(path)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(InstallerError::with_detail(
                "artifactSymlink",
                "Aseprite artifacts containing symbolic links are not accepted.",
                path.display().to_string(),
            ));
        }
        if !file_type.is_dir() && !file_type.is_file() {
            return Err(InstallerError::with_detail(
                "artifactSpecialFile",
                "Aseprite artifacts containing special files are not accepted.",
                path.display().to_string(),
            ));
        }

        let kind = if file_type.is_dir() { b'd' } else { b'f' };
        let relative_bytes = relative.as_os_str().as_bytes();
        if manifest.insert(relative_bytes.to_vec(), kind).is_some() {
            return Err(InstallerError::with_detail(
                "artifactCollision",
                "The Aseprite artifact contains duplicate paths.",
                path.display().to_string(),
            ));
        }
        hasher.update([kind]);
        hasher.update((relative_bytes.len() as u64).to_le_bytes());
        hasher.update(relative_bytes);
        hasher.update(metadata.mode().to_le_bytes());
        if file_type.is_file() {
            let mut file = open_regular_no_follow(path, "artifactFingerprint")?;
            let opened = file.metadata()?;
            let before = unix_snapshot(&opened);
            if before != unix_snapshot(&metadata) {
                return Err(InstallerError::with_detail(
                    "artifactChanged",
                    "The Aseprite artifact changed while it was being fingerprinted.",
                    path.display().to_string(),
                ));
            }
            hasher.update(opened.len().to_le_bytes());
            let mut bytes_read = 0_u64;
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                bytes_read = bytes_read.checked_add(count as u64).ok_or_else(|| {
                    InstallerError::new(
                        "artifactFingerprint",
                        "The Aseprite artifact is too large to fingerprint safely.",
                    )
                })?;
                hasher.update(&buffer[..count]);
            }
            let after = unix_snapshot(&file.metadata()?);
            let current = unix_snapshot(&std::fs::symlink_metadata(path)?);
            if bytes_read != opened.len() || before != after || after != current {
                return Err(InstallerError::with_detail(
                    "artifactChanged",
                    "The Aseprite artifact changed while it was being fingerprinted.",
                    path.display().to_string(),
                ));
            }
        } else {
            hasher.update(0_u64.to_le_bytes());
            directory_snapshots.push((path.to_path_buf(), unix_snapshot(&metadata)));
        }
    }
    for (path, expected) in directory_snapshots {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || unix_snapshot(&metadata) != expected
        {
            return Err(InstallerError::with_detail(
                "artifactChanged",
                "An Aseprite artifact directory changed while it was being fingerprinted.",
                path.display().to_string(),
            ));
        }
    }
    if unix_snapshot(&std::fs::symlink_metadata(root)?) != root_snapshot
        || collect_linux_artifact_manifest(root)? != manifest
    {
        return Err(InstallerError::with_detail(
            "artifactChanged",
            "The Aseprite artifact gained, lost, or changed an entry while it was being fingerprinted.",
            root.display().to_string(),
        ));
    }
    Ok(hasher.finalize().into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnixSnapshot {
    device: u64,
    inode: u64,
    mode: u32,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

fn unix_snapshot(metadata: &std::fs::Metadata) -> UnixSnapshot {
    UnixSnapshot {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        size: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    }
}

fn collect_linux_artifact_manifest(root: &Path) -> AppResult<BTreeMap<Vec<u8>, u8>> {
    let mut manifest = BTreeMap::new();
    let mut walker = WalkDir::new(root).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        let entry = entry.map_err(|error| {
            InstallerError::with_detail(
                "artifactFingerprint",
                "The complete Aseprite artifact tree could not be re-inspected.",
                error.to_string(),
            )
        })?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            walker.skip_current_dir();
            return Err(InstallerError::with_detail(
                "artifactSymlink",
                "Aseprite artifacts containing symbolic links are not accepted.",
                path.display().to_string(),
            ));
        }
        let kind = if metadata.is_dir() {
            b'd'
        } else if metadata.is_file() {
            b'f'
        } else {
            return Err(InstallerError::with_detail(
                "artifactSpecialFile",
                "Aseprite artifacts containing special files are not accepted.",
                path.display().to_string(),
            ));
        };
        let relative = path.strip_prefix(root).map_err(|error| {
            InstallerError::with_detail(
                "artifactPath",
                "The Aseprite artifact contains an invalid path.",
                error.to_string(),
            )
        })?;
        if manifest
            .insert(relative.as_os_str().as_bytes().to_vec(), kind)
            .is_some()
        {
            return Err(InstallerError::with_detail(
                "artifactCollision",
                "The Aseprite artifact contains duplicate paths.",
                path.display().to_string(),
            ));
        }
    }
    Ok(manifest)
}

pub fn validate_artifact(root: &Path, expected_version: &str) -> AppResult<String> {
    let observed = artifact_version(root)?;
    let expected = extract_version(expected_version).ok_or_else(|| {
        InstallerError::with_detail(
            "expectedVersion",
            "The expected Aseprite version is invalid.",
            expected_version,
        )
    })?;
    if observed != expected {
        return Err(InstallerError::with_detail(
            "artifactVersionMismatch",
            "The built executable does not match the selected Aseprite release.",
            format!("Expected {expected}; executable reported {observed}."),
        ));
    }
    Ok(observed)
}

/// Executes a structurally validated local Aseprite artifact only after the
/// caller has explicitly chosen to adopt or install it. Discovery must never
/// use this probe for untrusted candidates.
pub fn artifact_version(root: &Path) -> AppResult<String> {
    let root = std::fs::canonicalize(root).map_err(|error| {
        InstallerError::with_detail(
            "invalidBuild",
            "The built Aseprite artifact is unavailable.",
            format!("{}: {error}", root.display()),
        )
    })?;
    let metadata = std::fs::symlink_metadata(&root)?;
    if !metadata.file_type().is_dir() {
        return Err(InstallerError::with_detail(
            "invalidBuild",
            "The Linux Aseprite artifact must be a directory.",
            root.display().to_string(),
        ));
    }

    // Fingerprinting first forces inspection of the complete build/bin tree and
    // rejects links, devices, sockets and FIFOs before any produced code runs.
    artifact_fingerprint(&root)?;
    let executable = root.join("aseprite");
    validate_elf64_x86_64(&executable, true).map_err(|detail| {
        InstallerError::with_detail(
            "invalidElf",
            "The built Aseprite executable is not a Linux x86_64 ELF binary.",
            detail,
        )
    })?;

    for entry in WalkDir::new(&root).follow_links(false) {
        let entry = entry.map_err(|error| {
            InstallerError::with_detail(
                "invalidBuild",
                "The built Aseprite tree could not be validated.",
                error.to_string(),
            )
        })?;
        if !entry.file_type().is_file() || entry.path() == executable {
            continue;
        }
        if file_has_elf_magic(entry.path())? {
            validate_elf64_x86_64(entry.path(), false).map_err(|detail| {
                InstallerError::with_detail(
                    "invalidElf",
                    "The built Aseprite tree contains a binary for another architecture.",
                    detail,
                )
            })?;
        }
    }

    for required in ["data/gui.xml", "data/pref.xml"] {
        let path = root.join(required);
        if !path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_file())
        {
            return Err(InstallerError::with_detail(
                "invalidBuild",
                "The build completed without a complete Aseprite data directory.",
                path.display().to_string(),
            ));
        }
    }

    let output = run_sanitized_command(
        &executable,
        &[OsString::from("--version")],
        None,
        Some(&root),
        VERSION_TIMEOUT,
    )
    .map_err(|detail| {
        InstallerError::with_detail(
            "artifactVersion",
            "The built Aseprite executable did not pass its version check.",
            detail,
        )
    })?;
    let observed = extract_version(&output).ok_or_else(|| {
        InstallerError::with_detail(
            "artifactVersion",
            "The built Aseprite executable returned an unrecognized version.",
            output.trim(),
        )
    })?;
    Ok(observed)
}

pub fn find_built_artifact(build_dir: &Path) -> AppResult<PathBuf> {
    let direct = if build_dir.file_name() == Some(OsStr::new("bin")) {
        build_dir.to_path_buf()
    } else {
        build_dir.join("bin")
    };
    if is_artifact_layout(&direct) {
        return Ok(direct);
    }

    let mut candidates = if build_dir.is_dir() {
        WalkDir::new(build_dir)
            .max_depth(5)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_dir())
            .map(|entry| entry.into_path())
            .filter(|path| is_artifact_layout(path))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    candidates.sort();
    candidates.dedup();
    match candidates.as_slice() {
        [candidate] => Ok(candidate.clone()),
        [] => Err(InstallerError::with_detail(
            "invalidBuild",
            "The build completed without producing a Linux Aseprite artifact.",
            build_dir.display().to_string(),
        )),
        _ => Err(InstallerError::with_detail(
            "ambiguousBuild",
            "The build produced multiple Aseprite artifacts and none was selected.",
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        )),
    }
}

pub fn prepare_build_environment(minimum_cmake_version: [u64; 3]) -> AppResult<BuildEnvironment> {
    if std::env::consts::ARCH != "x86_64" {
        return Err(InstallerError::with_detail(
            "unsupportedArchitecture",
            "Linux builds are supported only on x86_64.",
            std::env::consts::ARCH,
        ));
    }
    let required_cmake = minimum_cmake_version.max(MINIMUM_CMAKE_VERSION);
    let clang = probe_versioned_tool("clang", &["--version"], MINIMUM_CLANG_VERSION, true);
    let clangxx = probe_versioned_tool("clang++", &["--version"], MINIMUM_CLANG_VERSION, true);
    let cmake = probe_versioned_tool("cmake", &["--version"], required_cmake, false);
    let ninja = probe_versioned_tool("ninja", &["--version"], MINIMUM_NINJA_VERSION, false);
    let environment =
        environment_from_probes(&clang, &clangxx, &cmake, &ninja).map_err(|detail| {
            InstallerError::with_detail(
                "buildEnvironment",
                "Clang, CMake and Ninja are not ready for an Aseprite build.",
                detail,
            )
        })?;

    let probe_parent = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("aseprite-installer");
    std::fs::create_dir_all(&probe_parent)?;
    let probe_root = probe_parent.join(format!(".aseprite-installer-toolchain-{}", Uuid::new_v4()));
    let result = functional_toolchain_probe(&probe_root, &environment);
    let _ = std::fs::remove_dir_all(&probe_root);
    result.map_err(|detail| {
        InstallerError::with_detail(
            "buildEnvironment",
            "The Linux C++17, X11, OpenGL and fontconfig toolchain is not functional.",
            detail,
        )
    })?;
    Ok(environment)
}

pub fn cmake_arguments(
    source: &Path,
    build: &Path,
    skia: &Path,
    environment: &BuildEnvironment,
) -> Vec<OsString> {
    let skia_library_dir = skia.join("out/Release-x64");
    vec![
        OsString::from("-S"),
        source.as_os_str().to_owned(),
        OsString::from("-B"),
        build.as_os_str().to_owned(),
        OsString::from("-G"),
        OsString::from("Ninja"),
        OsString::from("-DCMAKE_BUILD_TYPE:STRING=RelWithDebInfo"),
        joined_definition("CMAKE_MAKE_PROGRAM:FILEPATH", &environment.ninja),
        joined_definition("CMAKE_C_COMPILER:FILEPATH", &environment.clang),
        joined_definition("CMAKE_CXX_COMPILER:FILEPATH", &environment.clangxx),
        OsString::from("-DCMAKE_CXX_STANDARD:STRING=17"),
        OsString::from("-DCMAKE_CXX_STANDARD_REQUIRED:BOOL=ON"),
        OsString::from("-DCMAKE_CXX_EXTENSIONS:BOOL=OFF"),
        OsString::from("-DCMAKE_CXX_FLAGS:STRING=-stdlib=libstdc++"),
        OsString::from("-DCMAKE_EXE_LINKER_FLAGS:STRING=-stdlib=libstdc++"),
        OsString::from("-DCMAKE_EXPORT_COMPILE_COMMANDS:BOOL=OFF"),
        OsString::from("-DENABLE_CCACHE:BOOL=OFF"),
        OsString::from("-DENABLE_I18N_STRINGS:BOOL=OFF"),
        OsString::from("-DENABLE_TESTS:BOOL=OFF"),
        OsString::from("-DENABLE_BENCHMARKS:BOOL=OFF"),
        OsString::from("-DLAF_BACKEND:STRING=skia"),
        joined_definition("SKIA_DIR:PATH", skia),
        joined_definition("SKIA_LIBRARY_DIR:PATH", &skia_library_dir),
        joined_definition("SKIA_LIBRARY:FILEPATH", &skia_library_dir.join("libskia.a")),
    ]
}

pub fn target_aseprite_running(path: &Path) -> AppResult<bool> {
    let Some(executable) = resolve_executable(path) else {
        return Ok(false);
    };
    let expected = std::fs::canonicalize(&executable).map_err(|error| {
        InstallerError::with_detail(
            "processInspection",
            "The selected Aseprite executable could not be identified.",
            format!("{}: {error}", executable.display()),
        )
    })?;
    let effective_user = unsafe { libc::geteuid() };
    let processes = std::fs::read_dir("/proc").map_err(|error| {
        InstallerError::with_detail(
            "processInspection",
            "Running Linux processes could not be inspected.",
            error.to_string(),
        )
    })?;

    for process in processes {
        let process = match process {
            Ok(process) => process,
            Err(_) => continue,
        };
        if process
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .is_none()
        {
            continue;
        }
        let process_path = process.path();
        let metadata = match process_path.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.uid() != effective_user {
            continue;
        }
        let running = match std::fs::read_link(process_path.join("exe")) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(InstallerError::with_detail(
                    "processInspection",
                    "A process owned by the current user could not be identified safely.",
                    format!("{}: {error}", process_path.display()),
                ));
            }
            Err(_) => continue,
        };
        let running = strip_deleted_suffix(running);
        let running = std::fs::canonicalize(&running).unwrap_or(running);
        if running == expected {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn launch(path: &Path) -> AppResult<()> {
    let executable = validated_launch_executable(path)?;

    let (program, arguments) = if is_flatpak_aseprite_root(path) {
        (
            trusted_runtime_launcher("flatpak", "launch")?,
            vec![
                "run".to_owned(),
                "--".to_owned(),
                FLATPAK_ASEPRITE_ID.to_owned(),
            ],
        )
    } else if is_snap_aseprite_launcher(path) {
        (
            trusted_runtime_launcher("snap", "launch")?,
            vec!["run".to_owned(), "aseprite".to_owned()],
        )
    } else {
        (executable.clone(), Vec::new())
    };
    let mut command = tokio::process::Command::new(&program);
    sanitize_runtime_command(&mut command);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    if program == executable {
        command.current_dir(executable.parent().unwrap_or(Path::new("/")));
    }
    let mut child = command.spawn().map_err(|error| {
        InstallerError::with_detail(
            "launch",
            "Aseprite could not be launched.",
            format!("{}: {error}", program.display()),
        )
    })?;
    tauri::async_runtime::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}

fn validated_launch_executable(path: &Path) -> AppResult<PathBuf> {
    let visible_executable = resolve_executable(path).ok_or_else(|| {
        InstallerError::with_detail(
            "launch",
            "The selected Aseprite executable is missing.",
            path.display().to_string(),
        )
    })?;
    let executable = std::fs::canonicalize(&visible_executable).map_err(|error| {
        InstallerError::with_detail(
            "launch",
            "The selected Aseprite executable could not be resolved safely.",
            format!("{}: {error}", visible_executable.display()),
        )
    })?;
    validate_elf64_x86_64(&executable, true).map_err(|detail| {
        InstallerError::with_detail(
            "launch",
            "The selected file is not a valid Linux x86_64 Aseprite executable.",
            detail,
        )
    })?;
    Ok(executable)
}

pub async fn reveal(path: &Path) -> AppResult<()> {
    let visible = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let visible = std::fs::canonicalize(visible).map_err(|error| {
        InstallerError::with_detail(
            "reveal",
            "The selected installation location is unavailable.",
            format!("{}: {error}", visible.display()),
        )
    })?;
    let opener = trusted_desktop_opener("reveal")?;
    let mut command = tokio::process::Command::new(&opener);
    sanitize_runtime_command(&mut command);
    if opener.file_name() == Some(OsStr::new("gio")) {
        command.arg("open");
    }
    command
        .arg(&visible)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = tokio::time::timeout(Duration::from_secs(15), command.status())
        .await
        .map_err(|_| {
            InstallerError::new(
                "revealTimeout",
                "The desktop file manager did not accept the reveal request in time.",
            )
        })?
        .map_err(|error| {
            InstallerError::with_detail(
                "reveal",
                "The installation location could not be opened.",
                error.to_string(),
            )
        })?;
    if !status.success() {
        return Err(InstallerError::with_detail(
            "reveal",
            "The desktop file manager rejected the reveal request.",
            format!("{} exited with {status}", opener.display()),
        ));
    }
    Ok(())
}

pub async fn open_external(url: &str) -> AppResult<()> {
    let opener = trusted_desktop_opener("open")?;
    let mut command = tokio::process::Command::new(&opener);
    sanitize_runtime_command(&mut command);
    if opener.file_name() == Some(OsStr::new("gio")) {
        command.arg("open");
    }
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = tokio::time::timeout(Duration::from_secs(15), command.status())
        .await
        .map_err(|_| {
            InstallerError::new(
                "openTimeout",
                "The desktop opener did not accept the external URL in time.",
            )
        })?
        .map_err(|error| {
            InstallerError::with_detail(
                "open",
                "The external URL could not be opened.",
                error.to_string(),
            )
        })?;
    if !status.success() {
        return Err(InstallerError::with_detail(
            "open",
            "The desktop opener rejected the external URL.",
            format!("{} exited with {status}", opener.display()),
        ));
    }
    Ok(())
}

pub fn capture_desktop_integration(
    paths: &[PathBuf],
    installation_id: &str,
) -> AppResult<Vec<IntegrationSnapshot>> {
    if paths.is_empty() {
        validate_integration_id(installation_id)?;
        return Ok(Vec::new());
    }
    validate_integration_id(installation_id)?;
    let layout = validate_linux_integration_paths(paths, installation_id, true, false)?;
    let desktop = snapshot_regular_file(&layout.desktop_path, Some(0o644))?;
    let icon = snapshot_regular_file(&layout.icon_path, Some(0o644))?;
    if desktop.contents.is_some() && !desktop_entry_is_owned(&layout.desktop_path, installation_id)?
    {
        return Err(InstallerError::with_detail(
            "desktopIntegrationOwnership",
            "The desktop launcher path is occupied by a file not owned by this installer.",
            layout.desktop_path.display().to_string(),
        ));
    }
    match (&desktop.contents, &icon.contents) {
        (None, None) => {}
        (Some(_), Some(_)) => {
            let recorded = desktop_entry_icon_digest(&layout.desktop_path, installation_id)?;
            if recorded.as_deref() != icon.sha256.as_deref() {
                return Err(InstallerError::with_detail(
                    "desktopIntegrationOwnership",
                    "The desktop icon does not match the launcher's ownership digest.",
                    layout.icon_path.display().to_string(),
                ));
            }
        }
        _ => {
            return Err(InstallerError::new(
                "desktopIntegrationOwnership",
                "The managed desktop launcher and icon are only partially present.",
            ))
        }
    }
    snapshots_in_requested_order(paths, &[desktop, icon])
}

pub fn prepare_desktop_integration(
    root: &Path,
    installation_id: &str,
    paths: &[PathBuf],
) -> AppResult<Vec<IntegrationSnapshot>> {
    validate_integration_id(installation_id)?;
    let layout = validate_linux_integration_paths(paths, installation_id, true, true)?;
    let root = std::fs::canonicalize(root).map_err(|error| {
        InstallerError::with_detail(
            "desktopIntegration",
            "The installed Aseprite directory is unavailable.",
            format!("{}: {error}", root.display()),
        )
    })?;
    artifact_fingerprint(&root)?;
    let executable = root.join("aseprite");
    validate_elf64_x86_64(&executable, true).map_err(|detail| {
        InstallerError::with_detail(
            "desktopIntegration",
            "The installed Aseprite executable is invalid.",
            detail,
        )
    })?;
    let icon = find_png_icon(&root)?;
    let icon_bytes = std::fs::read(&icon)?;
    validate_png(&icon_bytes).map_err(|detail| {
        InstallerError::with_detail(
            "desktopIntegration",
            "The installed Aseprite icon is invalid.",
            detail,
        )
    })?;
    let icon_digest = hex::encode(Sha256::digest(&icon_bytes));

    let exec = desktop_exec_argument(&executable)?;
    let icon_value = desktop_plain_path(&layout.icon_path)?;
    let contents = format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Aseprite\nComment=Animated sprite editor\nExec={exec}\nIcon={icon_value}\nTerminal=false\nCategories=Graphics;2DGraphics;RasterGraphics;\nStartupNotify=true\nStartupWMClass=aseprite\nX-Aseprite-Installer-Id={installation_id}\nX-Aseprite-Installer-Icon-Sha256={icon_digest}\n"
    );
    let snapshots = [
        IntegrationSnapshot::file(layout.desktop_path, contents.into_bytes(), Some(0o644))?,
        IntegrationSnapshot::file(layout.icon_path, icon_bytes, Some(0o644))?,
    ];
    snapshots_in_requested_order(paths, &snapshots)
}

pub fn absent_desktop_integration(
    paths: &[PathBuf],
    installation_id: &str,
) -> AppResult<Vec<IntegrationSnapshot>> {
    validate_desktop_integration_paths(paths, installation_id)?;
    Ok(paths
        .iter()
        .cloned()
        .map(IntegrationSnapshot::absent)
        .collect())
}

pub fn apply_desktop_integration(
    desired: &[IntegrationSnapshot],
    alternative: &[IntegrationSnapshot],
    installation_id: &str,
) -> AppResult<Vec<PathBuf>> {
    let desired_paths = desired
        .iter()
        .map(|snapshot| snapshot.path.clone())
        .collect::<Vec<_>>();
    let alternative_paths = alternative
        .iter()
        .map(|snapshot| snapshot.path.clone())
        .collect::<Vec<_>>();
    validate_desktop_integration_paths(&desired_paths, installation_id)?;
    validate_desktop_integration_paths(&alternative_paths, installation_id)?;
    if !same_snapshot_paths(desired, alternative) {
        return Err(InstallerError::new(
            "desktopIntegrationSnapshot",
            "Desktop integration snapshots do not describe the same managed paths.",
        ));
    }
    if desired.is_empty() {
        return Ok(Vec::new());
    }
    let layout = validate_linux_integration_paths(&desired_paths, installation_id, true, true)?;
    ensure_integration_directory(&layout.data_root, layout.desktop_path.parent().unwrap())?;
    ensure_integration_directory(&layout.data_root, layout.icon_path.parent().unwrap())?;

    let mut ordered = desired.iter().collect::<Vec<_>>();
    if desired.iter().any(|snapshot| snapshot.contents.is_some()) {
        ordered.sort_by_key(|snapshot| (snapshot.path != layout.icon_path, snapshot.path.clone()));
    } else {
        ordered
            .sort_by_key(|snapshot| (snapshot.path != layout.desktop_path, snapshot.path.clone()));
    }
    for snapshot in ordered {
        snapshot.validate()?;
        let alternate = alternative
            .iter()
            .find(|candidate| candidate.path == snapshot.path)
            .ok_or_else(|| {
                InstallerError::new(
                    "desktopIntegrationSnapshot",
                    "A desktop integration alternative snapshot is missing.",
                )
            })?;
        apply_snapshot_file(snapshot, alternate)?;
    }
    Ok(desired
        .iter()
        .filter(|snapshot| snapshot.contents.is_some())
        .map(|snapshot| snapshot.path.clone())
        .collect())
}

fn snapshot_regular_file(path: &Path, mode: Option<u32>) -> AppResult<IntegrationSnapshot> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(IntegrationSnapshot::absent(path.to_path_buf()))
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(InstallerError::with_detail(
            "desktopIntegrationType",
            "A desktop integration path is not a regular non-link file.",
            path.display().to_string(),
        ));
    }
    if metadata.len() > MAX_INTEGRATION_FILE_BYTES as u64 {
        return Err(InstallerError::with_detail(
            "desktopIntegrationSize",
            "A desktop integration file exceeds the transaction snapshot safety limit.",
            path.display().to_string(),
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_INTEGRATION_FILE_BYTES + 1) as u64)
        .read_to_end(&mut contents)?;
    if contents.len() > MAX_INTEGRATION_FILE_BYTES {
        return Err(InstallerError::new(
            "desktopIntegrationSize",
            "A desktop integration file grew beyond its snapshot safety limit.",
        ));
    }
    IntegrationSnapshot::file(path.to_path_buf(), contents, mode)
}

fn apply_snapshot_file(
    desired: &IntegrationSnapshot,
    alternative: &IntegrationSnapshot,
) -> AppResult<()> {
    let current =
        snapshot_regular_file(&desired.path, desired.unix_mode.or(alternative.unix_mode))?;
    if current.contents == desired.contents && current.sha256 == desired.sha256 {
        return Ok(());
    }
    if current.contents != alternative.contents || current.sha256 != alternative.sha256 {
        return Err(InstallerError::with_detail(
            "desktopIntegrationConflict",
            "A desktop integration changed outside this transaction and was preserved.",
            desired.path.display().to_string(),
        ));
    }
    match desired.contents.as_deref() {
        Some(contents) => atomic_write(&desired.path, contents, desired.unix_mode.unwrap_or(0o644)),
        None => match std::fs::remove_file(&desired.path) {
            Ok(()) => {
                if let Some(parent) = desired.path.parent() {
                    File::open(parent)?.sync_all()?;
                }
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        },
    }
}

fn snapshots_in_requested_order(
    paths: &[PathBuf],
    snapshots: &[IntegrationSnapshot],
) -> AppResult<Vec<IntegrationSnapshot>> {
    paths
        .iter()
        .map(|path| {
            snapshots
                .iter()
                .find(|snapshot| &snapshot.path == path)
                .cloned()
                .ok_or_else(|| {
                    InstallerError::new(
                        "desktopIntegrationSnapshot",
                        "A desktop integration snapshot is missing.",
                    )
                })
        })
        .collect()
}

fn same_snapshot_paths(left: &[IntegrationSnapshot], right: &[IntegrationSnapshot]) -> bool {
    let mut left = left
        .iter()
        .map(|snapshot| &snapshot.path)
        .collect::<Vec<_>>();
    let mut right = right
        .iter()
        .map(|snapshot| &snapshot.path)
        .collect::<Vec<_>>();
    left.sort();
    right.sort();
    left == right
}

pub fn desktop_integration_paths(installation_id: &str) -> AppResult<Vec<PathBuf>> {
    validate_integration_id(installation_id)?;
    let data_home = xdg_data_home()?;
    let base_name = format!("{DESKTOP_PREFIX}{installation_id}");
    Ok(vec![
        data_home
            .join("applications")
            .join(format!("{base_name}.desktop")),
        data_home
            .join("aseprite-installer/icons")
            .join(format!("{base_name}.png")),
    ])
}

pub fn validate_desktop_integration_paths(
    paths: &[PathBuf],
    installation_id: &str,
) -> AppResult<()> {
    if paths.is_empty() {
        validate_integration_id(installation_id)?;
        return Ok(());
    }
    validate_linux_integration_paths(paths, installation_id, true, false).map(|_| ())
}

fn desktop_entry_icon_digest(path: &Path, installation_id: &str) -> AppResult<Option<String>> {
    validate_integration_id(installation_id)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    if file.metadata()?.len() > 64 * 1024 {
        return Ok(None);
    }
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let expected_id = format!("X-Aseprite-Installer-Id={installation_id}");
    if contents.lines().filter(|line| *line == expected_id).count() != 1 {
        return Ok(None);
    }
    let digests = contents
        .lines()
        .filter_map(|line| line.strip_prefix("X-Aseprite-Installer-Icon-Sha256="))
        .collect::<Vec<_>>();
    match digests.as_slice() {
        [digest] if digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
            Ok(Some(digest.to_ascii_lowercase()))
        }
        _ => Ok(None),
    }
}

fn desktop_entry_is_owned(path: &Path, installation_id: &str) -> AppResult<bool> {
    Ok(desktop_entry_icon_digest(path, installation_id)?.is_some())
}

fn discover(paths: &InstallerPaths, managed: &ManagedState) -> AppResult<Vec<InstallationInfo>> {
    let home = dirs::home_dir().ok_or_else(|| {
        InstallerError::new("home", "The current user home directory is unavailable.")
    })?;
    let mut candidates = BTreeMap::<PathBuf, InstallationChannel>::new();
    for record in &managed.installations {
        candidates.insert(PathBuf::from(&record.path), InstallationChannel::Managed);
    }
    candidates
        .entry(xdg_data_home()?.join("aseprite-installer/managed/Aseprite"))
        .or_insert(InstallationChannel::Manual);

    for directory in std::env::var_os("PATH")
        .as_deref()
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|directory| directory.is_absolute())
    {
        candidates
            .entry(directory.join("aseprite"))
            .or_insert(InstallationChannel::Manual);
    }
    for candidate in [
        PathBuf::from("/usr/bin/aseprite"),
        PathBuf::from("/usr/local/bin/aseprite"),
        PathBuf::from("/opt/aseprite"),
        PathBuf::from("/usr/local/share/aseprite"),
        PathBuf::from("/snap/bin/aseprite"),
    ] {
        candidates
            .entry(candidate)
            .or_insert(InstallationChannel::PackageManager);
    }

    for steam_root in steam_library_roots(&home) {
        candidates
            .entry(steam_root.join("steamapps/common/Aseprite"))
            .or_insert(InstallationChannel::Steam);
    }
    for flatpak in [
        home.join(".local/share/flatpak/app/org.aseprite.Aseprite/current/active/files"),
        PathBuf::from("/var/lib/flatpak/app/org.aseprite.Aseprite/current/active/files"),
    ] {
        candidates
            .entry(flatpak)
            .or_insert(InstallationChannel::PackageManager);
    }
    for executable in xdg_desktop_executables(&home) {
        candidates
            .entry(executable)
            .or_insert(InstallationChannel::Manual);
    }

    let mut results = Vec::new();
    let mut seen = BTreeSet::new();
    for (candidate, inferred_channel) in candidates {
        let Some(executable) = resolve_executable(&candidate) else {
            continue;
        };
        let executable = match std::fs::canonicalize(executable) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if validate_elf64_x86_64(&executable, true).is_err() || !seen.insert(executable.clone()) {
            continue;
        }
        if inferred_channel != InstallationChannel::Managed
            && is_internal_installer_artifact(paths, &candidate)
        {
            continue;
        }

        let record = managed.installations.iter().find(|record| {
            paths_equal(Path::new(&record.path), &candidate)
                || resolve_executable(Path::new(&record.path))
                    .and_then(|path| std::fs::canonicalize(path).ok())
                    .is_some_and(|path| path == executable)
        });
        let managed_record = record.filter(|record| managed_fingerprint_matches(record));
        let channel = if managed_record.is_some() {
            InstallationChannel::Managed
        } else {
            infer_channel(&candidate, inferred_channel)
        };
        let visible = managed_record
            .map(|record| PathBuf::from(&record.path))
            .unwrap_or_else(|| candidate.clone());
        let writable = is_writable_location(&visible);
        let manageable = managed_record.is_some()
            || (channel == InstallationChannel::Manual
                && visible.is_dir()
                && writable
                && artifact_fingerprint(&visible).is_ok());
        let visible_string = visible.to_string_lossy().into_owned();
        let backup = managed_record
            .and_then(|record| record.backup_path.as_deref())
            .map(Path::new)
            .is_some_and(|backup| backup.exists());
        results.push(InstallationInfo {
            id: managed_record
                .map(|record| record.id.clone())
                .unwrap_or_else(|| installation_id(&visible_string)),
            path: visible_string,
            version: managed_record
                .and_then(|record| record.source_version.clone())
                .or_else(|| managed_record.map(|record| record.tag.clone())),
            version_exact: managed_record
                .map(|record| record.version_exact)
                .unwrap_or(false),
            architecture: Some("x86_64".into()),
            channel,
            manageable,
            writable,
            has_backup: backup,
            installed_at: managed_record.map(|record| record.installed_at.clone()),
        });
    }
    results.sort_by(|left, right| {
        channel_rank(&left.channel)
            .cmp(&channel_rank(&right.channel))
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(results)
}

fn run_preflight(paths: &InstallerPaths, context: &PreflightContext) -> AppResult<PreflightReport> {
    let effective_user = unsafe { libc::geteuid() };
    let non_elevated = effective_user != 0;
    let architecture = std::env::consts::ARCH.to_owned();
    let supported_architecture = architecture == "x86_64";
    let distribution = linux_distribution();
    let os_version = distribution.pretty_name.clone();
    let package_help = distro_install_hint(&distribution);
    let required_cmake = context.minimum_cmake_version.max(MINIMUM_CMAKE_VERSION);
    let clang = probe_versioned_tool("clang", &["--version"], MINIMUM_CLANG_VERSION, true);
    let clangxx = probe_versioned_tool("clang++", &["--version"], MINIMUM_CLANG_VERSION, true);
    let cmake = probe_versioned_tool("cmake", &["--version"], required_cmake, false);
    let ninja = probe_versioned_tool("ninja", &["--version"], MINIMUM_NINJA_VERSION, false);
    let environment = environment_from_probes(&clang, &clangxx, &cmake, &ninja);

    let workspace = if non_elevated {
        probe_workspace(paths)
    } else {
        Err("Skipped while the installer is running as root.".into())
    };
    let destination_parent = context
        .target
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            InstallerError::new("target", "The installation target has no parent directory.")
        })?;
    let destination = if non_elevated {
        probe_directory_capabilities(&destination_parent, "installation destination")
    } else {
        Err("Skipped while the installer is running as root.".into())
    };
    let target_state = probe_target_state(&context.target);
    let target_closed = target_aseprite_running(&context.target)
        .map(|running| {
            if running {
                Err("The selected Aseprite executable is currently running.".into())
            } else {
                Ok("No process is using the selected Aseprite executable.".into())
            }
        })
        .unwrap_or_else(|error| Err(error.detail.unwrap_or(error.message)));

    let build_toolchain = if supported_architecture && non_elevated && workspace.is_ok() {
        match &environment {
            Ok(environment) => functional_toolchain_probe(&paths.builds_dir, environment),
            Err(error) => Err(error.clone()),
        }
    } else {
        Err("Resolve the architecture, session and workspace checks first.".into())
    };
    let destination_execution = if destination.is_ok() {
        match &environment {
            Ok(environment) => executable_volume_probe(&destination_parent, environment),
            Err(error) => Err(error.clone()),
        }
    } else {
        Err("Resolve destination permissions before testing executable files there.".into())
    };

    let cache_space = available_space(&paths.cache_dir);
    let destination_space = available_space(&destination_parent);
    let backup_space = available_space(&paths.backups_dir);
    let free_bytes = match (&cache_space, &destination_space, &backup_space) {
        (Ok(cache), Ok(destination), Ok(backup)) => (*cache).min(*destination).min(*backup),
        _ => 0,
    };
    let space_ok = cache_space.is_ok()
        && destination_space.is_ok()
        && backup_space.is_ok()
        && cache_space
            .as_ref()
            .is_ok_and(|bytes| *bytes >= BUILD_SAFETY_BUDGET_BYTES)
        && destination_space
            .as_ref()
            .is_ok_and(|bytes| *bytes >= BUILD_SAFETY_BUDGET_BYTES)
        && backup_space
            .as_ref()
            .is_ok_and(|bytes| *bytes >= BUILD_SAFETY_BUDGET_BYTES);

    let prerequisites = vec![
        Prerequisite {
            id: "nonElevated".into(),
            label: "Normal user session".into(),
            ok: non_elevated,
            required: true,
            detail: format!("Effective user ID: {effective_user}"),
            remediation: (!non_elevated).then(|| {
                "Quit this elevated copy and reopen Aseprite Installer as your normal desktop user. The installer never invokes sudo or pkexec.".into()
            }),
        },
        Prerequisite {
            id: "linux".into(),
            label: "Linux distribution".into(),
            ok: true,
            required: true,
            detail: format!("{} · {package_help}", distribution.pretty_name),
            remediation: None,
        },
        Prerequisite {
            id: "architecture".into(),
            label: "x86_64 build architecture".into(),
            ok: supported_architecture,
            required: true,
            detail: architecture.clone(),
            remediation: (!supported_architecture).then(|| {
                "Use the x86_64 Linux installer. Linux ARM64 is not supported because this workflow requires the official matching Aseprite Skia package.".into()
            }),
        },
        prerequisite_for_tool(
            "clang",
            "Clang and libstdc++",
            &[&clang, &clangxx],
            &package_help,
        ),
        prerequisite_for_tool("cmake", "CMake", &[&cmake], &package_help),
        prerequisite_for_tool("ninja", "Ninja", &[&ninja], &package_help),
        Prerequisite {
            id: "workspace".into(),
            label: "Installer storage permissions".into(),
            ok: workspace.is_ok(),
            required: true,
            detail: workspace.clone().unwrap_or_else(|error| error),
            remediation: workspace.as_ref().err().map(|_| {
                "Restore write and rename access to the installer data/cache directories, then check again. Do not run the installer with sudo.".into()
            }),
        },
        Prerequisite {
            id: "destination".into(),
            label: "Installation destination".into(),
            ok: destination.is_ok() && target_state.is_ok(),
            required: true,
            detail: match (&destination, &target_state) {
                (Ok(first), Ok(second)) => format!("{first} {second}"),
                (Err(error), _) | (_, Err(error)) => error.clone(),
            },
            remediation: (destination.is_err() || target_state.is_err()).then(|| {
                "Use the default per-user destination or another absolute, user-writable directory. Remove any symbolic link at the target; no administrator access is needed.".into()
            }),
        },
        Prerequisite {
            id: "toolchain".into(),
            label: "C++17 and Linux desktop libraries".into(),
            ok: build_toolchain.is_ok(),
            required: true,
            detail: build_toolchain.clone().unwrap_or_else(|error| error),
            remediation: build_toolchain.as_ref().err().map(|_| {
                format!("Install the missing C++17/X11/Xcursor/Xi/Xrandr/OpenGL/fontconfig development packages, then check again. {package_help}")
            }),
        },
        Prerequisite {
            id: "executableDestination".into(),
            label: "Executable destination volume".into(),
            ok: destination_execution.is_ok(),
            required: true,
            detail: destination_execution
                .clone()
                .unwrap_or_else(|error| error),
            remediation: destination_execution.as_ref().err().map(|_| {
                "Choose a destination mounted with executable files enabled; remove a noexec mount restriction or use the default per-user data directory.".into()
            }),
        },
        Prerequisite {
            id: "asepriteClosed".into(),
            label: "Selected Aseprite is closed".into(),
            ok: target_closed.is_ok(),
            required: true,
            detail: target_closed.clone().unwrap_or_else(|error| error),
            remediation: target_closed.as_ref().err().map(|_| {
                "Quit the selected Aseprite process, then check again. A reboot is not required.".into()
            }),
        },
        Prerequisite {
            id: "disk".into(),
            label: "Build and install space safety budget".into(),
            ok: space_ok,
            required: true,
            detail: match (&cache_space, &destination_space, &backup_space) {
                (Ok(cache), Ok(destination), Ok(backup)) => format!(
                    "Cache: {:.1} GB · destination: {:.1} GB · backups: {:.1} GB",
                    *cache as f64 / 1024_f64.powi(3),
                    *destination as f64 / 1024_f64.powi(3),
                    *backup as f64 / 1024_f64.powi(3)
                ),
                _ => "Free space could not be inspected on every build, destination, and backup volume.".into(),
            },
            remediation: (!space_ok).then(|| {
                "Free the required 6 GB safety budget on the cache, destination, and backup volumes. Exact capacity is checked again before every copy or mutation.".into()
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
        homebrew_available: false,
        prerequisites,
    })
}

fn prerequisite_for_tool(
    id: &str,
    label: &str,
    probes: &[&ToolProbe],
    package_help: &str,
) -> Prerequisite {
    let ok = probes.iter().all(|probe| probe.path.is_some());
    Prerequisite {
        id: id.into(),
        label: label.into(),
        ok,
        required: true,
        detail: probes
            .iter()
            .map(|probe| probe.detail.as_str())
            .collect::<Vec<_>>()
            .join(" · "),
        remediation: (!ok).then(|| {
            format!("Install the required tool from your distribution, then check again. {package_help}")
        }),
    }
}

fn probe_workspace(paths: &InstallerPaths) -> Result<String, String> {
    for (directory, label) in [
        (&paths.data_dir, "installer data"),
        (&paths.cache_dir, "installer cache"),
        (&paths.archives_dir, "archive cache"),
        (&paths.builds_dir, "build workspace"),
        (&paths.logs_dir, "installer logs"),
        (&paths.backups_dir, "installation backups"),
    ] {
        probe_directory_capabilities(directory, label)?;
    }
    Ok("All installer directories passed write, fsync, and atomic rename probes.".into())
}

fn probe_directory_capabilities(directory: &Path, label: &str) -> Result<String, String> {
    if !directory.is_absolute() {
        return Err(format!(
            "The {label} path is not absolute: {}",
            directory.display()
        ));
    }
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
    let metadata = std::fs::symlink_metadata(directory)
        .map_err(|error| format!("Could not inspect {}: {error}", directory.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "The {label} path is not a real directory: {}",
            directory.display()
        ));
    }
    let probe = directory.join(format!(
        ".aseprite-installer-directory-probe-{}",
        Uuid::new_v4()
    ));
    std::fs::create_dir(&probe).map_err(|error| {
        format!(
            "Could not create a probe in {}: {error}",
            directory.display()
        )
    })?;
    let first = probe.join("first");
    let second = probe.join("second");
    let result: Result<(), String> = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&first)
            .map_err(|error| format!("Could not write in {}: {error}", directory.display()))?;
        file.write_all(b"aseprite-installer-directory-probe")
            .map_err(|error| format!("Could not write in {}: {error}", directory.display()))?;
        file.sync_all()
            .map_err(|error| format!("Could not fsync in {}: {error}", directory.display()))?;
        std::fs::rename(&first, &second)
            .map_err(|error| format!("Could not rename in {}: {error}", directory.display()))?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&first);
    let _ = std::fs::remove_file(&second);
    let _ = std::fs::remove_dir(&probe);
    result?;
    Ok(format!(
        "{} supports regular-file writes, fsync, and same-directory rename.",
        directory.display()
    ))
}

fn probe_target_state(target: &Path) -> Result<String, String> {
    if !target.is_absolute() || target.parent().is_none() {
        return Err("The installation target must be an absolute path with a parent.".into());
    }
    match std::fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "The target is a symbolic link and cannot be replaced safely: {}",
            target.display()
        )),
        Ok(metadata) if !metadata.is_dir() => Err(format!(
            "The target exists but is not an Aseprite directory: {}",
            target.display()
        )),
        Ok(_) => Ok("The existing target is a real directory.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok("The target is available for a new installation.".into())
        }
        Err(error) => Err(format!("Could not inspect {}: {error}", target.display())),
    }
}

fn functional_toolchain_probe(
    directory: &Path,
    environment: &BuildEnvironment,
) -> Result<String, String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
    let probe = directory.join(format!(".aseprite-installer-cxx-probe-{}", Uuid::new_v4()));
    std::fs::create_dir(&probe)
        .map_err(|error| format!("Could not create {}: {error}", probe.display()))?;
    let source = probe.join("probe.cpp");
    let executable = probe.join("probe");
    let source_code = br#"#include <filesystem>
#include <X11/Xlib.h>
#include <X11/Xcursor/Xcursor.h>
#include <X11/extensions/XInput2.h>
#include <X11/extensions/Xrandr.h>
#include <GL/gl.h>
#include <fontconfig/fontconfig.h>
int main() {
  volatile auto x11 = &XOpenDisplay;
  volatile auto cursor = &XcursorImageCreate;
  volatile auto xi = &XIQueryVersion;
  volatile auto randr = &XRRQueryVersion;
  volatile auto gl = &glGetString;
  std::filesystem::path p{"."};
  return (x11 && cursor && xi && randr && gl && !p.empty() && FcInit()) ? 0 : 1;
}
"#;
    let result = (|| {
        atomic_write(&source, source_code, 0o600).map_err(|error| error.to_string())?;
        let args = vec![
            OsString::from("-std=c++17"),
            OsString::from("-stdlib=libstdc++"),
            OsString::from("-Wl,--no-as-needed"),
            source.as_os_str().to_owned(),
            OsString::from("-o"),
            executable.as_os_str().to_owned(),
            OsString::from("-lX11"),
            OsString::from("-lXcursor"),
            OsString::from("-lXi"),
            OsString::from("-lXrandr"),
            OsString::from("-lGL"),
            OsString::from("-lfontconfig"),
        ];
        run_sanitized_command(
            &environment.clangxx,
            &args,
            Some(environment),
            Some(&probe),
            COMMAND_TIMEOUT,
        )?;
        validate_elf64_x86_64(&executable, true)?;
        run_sanitized_command(
            &executable,
            &[],
            Some(environment),
            Some(&probe),
            COMMAND_TIMEOUT,
        )?;
        Ok(format!(
            "Clang C++17 with libstdc++, X11, Xcursor, Xi, Xrandr, OpenGL and fontconfig compiled and ran in {}.",
            directory.display()
        ))
    })();
    let _ = std::fs::remove_dir_all(&probe);
    result
}

fn executable_volume_probe(
    directory: &Path,
    environment: &BuildEnvironment,
) -> Result<String, String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
    let probe = directory.join(format!(".aseprite-installer-exec-probe-{}", Uuid::new_v4()));
    std::fs::create_dir(&probe)
        .map_err(|error| format!("Could not create {}: {error}", probe.display()))?;
    let source = probe.join("probe.cpp");
    let executable = probe.join("probe");
    let result = (|| {
        atomic_write(&source, b"int main() { return 0; }\n", 0o600)
            .map_err(|error| error.to_string())?;
        run_sanitized_command(
            &environment.clangxx,
            &[
                OsString::from("-std=c++17"),
                OsString::from("-stdlib=libstdc++"),
                source.as_os_str().to_owned(),
                OsString::from("-o"),
                executable.as_os_str().to_owned(),
            ],
            Some(environment),
            Some(&probe),
            COMMAND_TIMEOUT,
        )?;
        validate_elf64_x86_64(&executable, true)?;
        run_sanitized_command(
            &executable,
            &[],
            Some(environment),
            Some(&probe),
            COMMAND_TIMEOUT,
        )?;
        Ok(format!(
            "Compiled ELF executables run from {}.",
            directory.display()
        ))
    })();
    let _ = std::fs::remove_dir_all(&probe);
    result
}

fn probe_versioned_tool(
    name: &str,
    arguments: &[&str],
    minimum: [u64; 3],
    require_clang_identity: bool,
) -> ToolProbe {
    let path = match find_tool(name) {
        Ok(path) => path,
        Err(detail) => {
            return ToolProbe {
                path: None,
                version: None,
                detail,
            }
        }
    };
    let args = arguments.iter().map(OsString::from).collect::<Vec<_>>();
    let output = match run_sanitized_command(&path, &args, None, None, COMMAND_TIMEOUT) {
        Ok(output) => output,
        Err(detail) => {
            return ToolProbe {
                path: None,
                version: None,
                detail: format!("{} failed its version probe: {detail}", path.display()),
            }
        }
    };
    if require_clang_identity && !output.to_ascii_lowercase().contains("clang") {
        return ToolProbe {
            path: None,
            version: None,
            detail: format!("{} did not identify itself as Clang.", path.display()),
        };
    }
    let Some(version) = parse_numeric_version(&output) else {
        return ToolProbe {
            path: None,
            version: None,
            detail: format!(
                "{} returned an unrecognized version: {output}",
                path.display()
            ),
        };
    };
    if !version_at_least(version, minimum) {
        return ToolProbe {
            path: None,
            version: Some(version),
            detail: format!(
                "{} is {} ({} or newer is required).",
                path.display(),
                format_version(version),
                format_version(minimum)
            ),
        };
    }
    ToolProbe {
        path: Some(path.clone()),
        version: Some(version),
        detail: format!("{} · {}", path.display(), format_version(version)),
    }
}

fn environment_from_probes(
    clang: &ToolProbe,
    clangxx: &ToolProbe,
    cmake: &ToolProbe,
    ninja: &ToolProbe,
) -> Result<BuildEnvironment, String> {
    let missing = [clang, clangxx, cmake, ninja]
        .iter()
        .filter(|probe| probe.path.is_none())
        .map(|probe| probe.detail.as_str())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(missing.join(" "));
    }
    if clang.version.map(|version| version[0]) != clangxx.version.map(|version| version[0]) {
        return Err(format!(
            "clang and clang++ must use the same major release. {} · {}",
            clang.detail, clangxx.detail
        ));
    }
    let clang = clang.path.clone().expect("checked above");
    let clangxx = clangxx.path.clone().expect("checked above");
    let cmake = cmake.path.clone().expect("checked above");
    let ninja = ninja.path.clone().expect("checked above");
    let mut path_directories = Vec::new();
    for tool in [&clang, &clangxx, &cmake, &ninja] {
        if let Some(parent) = tool.parent() {
            if !path_directories.iter().any(|existing| existing == parent) {
                path_directories.push(parent.to_path_buf());
            }
        }
    }
    for system in ["/usr/local/bin", "/usr/bin", "/bin"] {
        let system = PathBuf::from(system);
        if !path_directories.contains(&system) {
            path_directories.push(system);
        }
    }
    let path = std::env::join_paths(path_directories).map_err(|error| error.to_string())?;
    Ok(BuildEnvironment {
        path,
        clang,
        clangxx,
        cmake,
        ninja,
        home_dir: PathBuf::from("/nonexistent/aseprite-installer-build-home"),
        temporary_dir: PathBuf::from("/tmp"),
    })
}

fn find_tool(name: &str) -> Result<PathBuf, String> {
    let mut directories = std::env::var_os("PATH")
        .as_deref()
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|path| path.is_absolute())
        .collect::<Vec<_>>();
    for known in ["/usr/local/bin", "/usr/bin", "/bin"] {
        let path = PathBuf::from(known);
        if !directories.contains(&path) {
            directories.push(path);
        }
    }
    for directory in directories {
        let candidate = directory.join(name);
        if !executable_regular_file(&candidate) {
            continue;
        }
        let canonical = std::fs::canonicalize(&candidate)
            .map_err(|error| format!("Could not resolve {}: {error}", candidate.display()))?;
        if executable_regular_file(&canonical) {
            return Ok(canonical);
        }
    }
    Err(format!(
        "{name} was not found as an executable regular file in PATH."
    ))
}

fn run_sanitized_command(
    program: &Path,
    arguments: &[OsString],
    environment: Option<&BuildEnvironment>,
    current_dir: Option<&Path>,
    timeout: Duration,
) -> Result<String, String> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(environment) = environment {
        environment.configure_std(&mut command);
    } else {
        command
            .env_clear()
            .env("PATH", SYSTEM_PATH)
            .env("HOME", "/nonexistent/aseprite-installer-probe-home")
            .env("TMPDIR", "/tmp")
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8");
    }
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start {}: {error}", program.display()))?;
    let process_id = child.id();
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            kill_process_group(process_id);
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Could not capture {} stdout.", program.display()));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            kill_process_group(process_id);
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Could not capture {} stderr.", program.display()));
        }
    };
    let stdout_reader = std::thread::spawn(move || read_capped_output(stdout));
    let stderr_reader = std::thread::spawn(move || read_capped_output(stderr));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                kill_process_group(process_id);
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!(
                    "{} did not finish within {} seconds.",
                    program.display(),
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                kill_process_group(process_id);
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("Could not wait for {}: {error}", program.display()));
            }
        }
    };
    // A successful direct process must not leave descendants holding inherited
    // output pipes open. The command was placed in its own process group, so
    // this closes any stragglers before joining the bounded readers.
    kill_process_group(process_id);
    let (stdout, stdout_exceeded) = stdout_reader
        .join()
        .map_err(|_| format!("Could not read {} stdout.", program.display()))?
        .map_err(|error| format!("Could not read {} stdout: {error}", program.display()))?;
    let (stderr, stderr_exceeded) = stderr_reader
        .join()
        .map_err(|_| format!("Could not read {} stderr.", program.display()))?
        .map_err(|error| format!("Could not read {} stderr: {error}", program.display()))?;
    if stdout_exceeded
        || stderr_exceeded
        || stdout.len().saturating_add(stderr.len()) > COMMAND_OUTPUT_LIMIT_BYTES
    {
        return Err(format!("{} produced excessive output.", program.display()));
    }
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    let combined = format!(
        "{}{}{}",
        stdout,
        if stdout.is_empty() || stderr.is_empty() {
            ""
        } else {
            "\n"
        },
        stderr
    );
    if !status.success() {
        return Err(format!(
            "{} exited with {status}: {}",
            program.display(),
            combined.trim()
        ));
    }
    Ok(combined.trim().to_owned())
}

fn read_capped_output<R: Read>(mut reader: R) -> std::io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::new();
    let mut exceeded = false;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = COMMAND_OUTPUT_LIMIT_BYTES.saturating_sub(kept.len());
        let accepted = remaining.min(count);
        kept.extend_from_slice(&buffer[..accepted]);
        exceeded |= accepted < count;
    }
    Ok((kept, exceeded))
}

fn kill_process_group(process_id: u32) {
    if let Ok(process_id) = i32::try_from(process_id) {
        // SAFETY: a negative PID targets only the child-created process group.
        unsafe {
            libc::kill(-process_id, libc::SIGKILL);
        }
    }
}

fn sanitize_runtime_command(command: &mut tokio::process::Command) {
    for variable in DANGEROUS_RUNTIME_VARIABLES {
        command.env_remove(variable);
    }
    for (variable, _) in std::env::vars_os() {
        let name = variable.to_string_lossy();
        if name.starts_with("BASH_FUNC_") || name.starts_with("GIT_CONFIG_") {
            command.env_remove(variable);
        }
    }
}

fn validate_elf64_x86_64(path: &Path, require_executable: bool) -> Result<(), String> {
    let mut file = open_regular_no_follow(path, "elf")
        .map_err(|error| error.detail.unwrap_or(error.message))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if require_executable && metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!("{} is not marked executable.", path.display()));
    }
    let mut header = [0_u8; 64];
    file.read_exact(&mut header).map_err(|error| {
        format!(
            "Could not read the ELF header in {}: {error}",
            path.display()
        )
    })?;
    if &header[..4] != b"\x7fELF" {
        return Err(format!("{} does not have an ELF header.", path.display()));
    }
    if header[4] != 2 {
        return Err(format!("{} is not ELF64.", path.display()));
    }
    if header[5] != 1 {
        return Err(format!("{} is not little-endian ELF.", path.display()));
    }
    if header[6] != 1 {
        return Err(format!(
            "{} has an unsupported ELF version.",
            path.display()
        ));
    }
    let elf_type = u16::from_le_bytes([header[16], header[17]]);
    if !matches!(elf_type, 2 | 3) {
        return Err(format!(
            "{} is neither an ELF executable nor a shared object.",
            path.display()
        ));
    }
    let machine = u16::from_le_bytes([header[18], header[19]]);
    if machine != 62 {
        return Err(format!(
            "{} targets ELF machine {machine}, not x86_64 (62).",
            path.display()
        ));
    }
    let elf_version = u32::from_le_bytes([header[20], header[21], header[22], header[23]]);
    if elf_version != 1 {
        return Err(format!(
            "{} has an invalid ELF header version.",
            path.display()
        ));
    }
    Ok(())
}

fn file_has_elf_magic(path: &Path) -> AppResult<bool> {
    let mut file = open_regular_no_follow(path, "elfInspection")?;
    let mut magic = [0_u8; 4];
    match file.read_exact(&mut magic) {
        Ok(()) => Ok(magic == *b"\x7fELF"),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn open_regular_no_follow(path: &Path, code: &str) -> AppResult<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| {
            InstallerError::with_detail(
                code,
                "A file could not be opened without following links.",
                format!("{}: {error}", path.display()),
            )
        })?;
    if !file.metadata()?.is_file() {
        return Err(InstallerError::with_detail(
            code,
            "A required path is not a regular file.",
            path.display().to_string(),
        ));
    }
    Ok(file)
}

fn is_artifact_layout(path: &Path) -> bool {
    path.symlink_metadata()
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        && path.join("aseprite").is_file()
        && path.join("data/gui.xml").is_file()
        && path.join("data/pref.xml").is_file()
}

fn resolve_executable(path: &Path) -> Option<PathBuf> {
    if path.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Some(path.to_path_buf());
    }
    for relative in ["aseprite", "bin/aseprite", "files/bin/aseprite"] {
        let candidate = path.join(relative);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn extract_version(value: &str) -> Option<String> {
    value
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .filter(|candidate| {
            candidate.contains('.')
                && candidate.split('.').all(|part| {
                    !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
                })
        })
        .max_by_key(|candidate| candidate.split('.').count())
        .map(str::to_owned)
}

fn parse_numeric_version(value: &str) -> Option<[u64; 3]> {
    let candidate = value
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .find(|candidate| {
            !candidate.is_empty()
                && candidate.split('.').all(|part| {
                    !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
                })
        })?;
    let mut parts = candidate
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok());
    Some([
        parts.next()?,
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    ])
}

fn version_at_least(actual: [u64; 3], minimum: [u64; 3]) -> bool {
    actual >= minimum
}

fn format_version(version: [u64; 3]) -> String {
    format!("{}.{}.{}", version[0], version[1], version[2])
}

fn joined_definition(name: &str, value: &Path) -> OsString {
    let mut result = OsString::from(format!("-D{name}="));
    result.push(value.as_os_str());
    result
}

fn xdg_data_home() -> AppResult<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return Ok(path);
        }
    }
    dirs::home_dir()
        .map(|home| home.join(".local/share"))
        .ok_or_else(|| {
            InstallerError::new("home", "The current user data directory is unavailable.")
        })
}

fn xdg_desktop_executables(home: &Path) -> Vec<PathBuf> {
    let mut application_directories = BTreeSet::new();
    if let Ok(data_home) = xdg_data_home() {
        application_directories.insert(data_home.join("applications"));
    }
    let data_directories = std::env::var_os("XDG_DATA_DIRS")
        .as_deref()
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|path| path.is_absolute())
        .collect::<Vec<_>>();
    if data_directories.is_empty() {
        application_directories.insert(PathBuf::from("/usr/local/share/applications"));
        application_directories.insert(PathBuf::from("/usr/share/applications"));
    } else {
        application_directories.extend(
            data_directories
                .into_iter()
                .map(|directory| directory.join("applications")),
        );
    }
    application_directories.insert(home.join(".local/share/flatpak/exports/share/applications"));
    application_directories.insert(PathBuf::from("/var/lib/flatpak/exports/share/applications"));

    let mut inspected = 0_usize;
    let mut executables = BTreeSet::new();
    'directories: for directory in application_directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            if inspected >= MAX_DISCOVERY_DESKTOP_ENTRIES {
                break 'directories;
            }
            inspected += 1;
            let path = entry.path();
            if !path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("desktop"))
            {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_DISCOVERY_DESKTOP_ENTRY_BYTES
            {
                continue;
            }
            let Ok(mut file) = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&path)
            else {
                continue;
            };
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            if Read::by_ref(&mut file)
                .take(MAX_DISCOVERY_DESKTOP_ENTRY_BYTES + 1)
                .read_to_end(&mut bytes)
                .is_err()
                || bytes.len() as u64 > MAX_DISCOVERY_DESKTOP_ENTRY_BYTES
            {
                continue;
            }
            let Ok(contents) = std::str::from_utf8(&bytes) else {
                continue;
            };
            if let Some(executable) = parse_desktop_entry_executable(
                contents,
                entry.file_name().to_string_lossy().as_ref(),
            ) {
                executables.insert(executable);
            }
        }
    }
    executables.into_iter().collect()
}

fn parse_desktop_entry_executable(contents: &str, file_name: &str) -> Option<PathBuf> {
    let mut in_desktop_entry = false;
    let mut entry_type = None;
    let mut name = None;
    let mut hidden = false;
    let mut exec = None;
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_entry || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "Type" => entry_type = Some(value.trim()),
            "Name" => name = Some(value.trim()),
            "Hidden" => hidden = value.trim().eq_ignore_ascii_case("true"),
            "Exec" => exec = Some(value.trim()),
            _ => {}
        }
    }
    if entry_type != Some("Application")
        || hidden
        || (!file_name.to_ascii_lowercase().contains("aseprite")
            && !name.is_some_and(|name| name.to_ascii_lowercase().contains("aseprite")))
    {
        return None;
    }
    let tokens = parse_desktop_exec_tokens(exec?)?;
    desktop_literal_executable_path(tokens.first()?)
}

fn desktop_literal_executable_path(token: &str) -> Option<PathBuf> {
    let mut literal = String::with_capacity(token.len());
    let mut characters = token.chars();
    while let Some(character) = characters.next() {
        if character == '%' {
            if characters.next() != Some('%') {
                return None;
            }
            literal.push('%');
        } else {
            literal.push(character);
        }
    }
    let path = PathBuf::from(literal);
    let file_name = path.file_name()?.to_string_lossy();
    let lowercase = file_name.to_ascii_lowercase();
    let aseprite_binary = file_name == "aseprite";
    let aseprite_appimage = lowercase.contains("aseprite") && lowercase.ends_with(".appimage");
    (path.is_absolute() && (aseprite_binary || aseprite_appimage)).then_some(path)
}

fn parse_desktop_exec_tokens(value: &str) -> Option<Vec<String>> {
    if value.contains(['\0', '\n', '\r']) {
        return None;
    }
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            token.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character.is_whitespace() && !quoted {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
        } else {
            token.push(character);
        }
    }
    if escaped || quoted {
        return None;
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    (!tokens.is_empty()).then_some(tokens)
}

fn linux_distribution() -> LinuxDistribution {
    let contents = ["/etc/os-release", "/usr/lib/os-release"]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let values = parse_os_release(&contents);
    LinuxDistribution {
        id: values
            .get("ID")
            .cloned()
            .unwrap_or_else(|| "unknown".into()),
        id_like: values.get("ID_LIKE").cloned().unwrap_or_default(),
        pretty_name: values
            .get("PRETTY_NAME")
            .cloned()
            .unwrap_or_else(|| "Linux".into()),
    }
}

fn parse_os_release(contents: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        if !key
            .chars()
            .all(|character| character.is_ascii_uppercase() || character == '_')
        {
            continue;
        }
        let raw = raw.trim();
        let value = raw
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(raw)
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
        values.insert(key.to_owned(), value);
    }
    values
}

fn distro_install_hint(distribution: &LinuxDistribution) -> String {
    let family = format!("{} {}", distribution.id, distribution.id_like).to_ascii_lowercase();
    if family.contains("debian") || family.contains("ubuntu") {
        "Debian/Ubuntu packages: clang g++ cmake ninja-build libx11-dev libxcursor-dev libxi-dev libxrandr-dev libgl1-mesa-dev libfontconfig1-dev".into()
    } else if family.contains("fedora") || family.contains("rhel") {
        "Fedora/RHEL packages: clang gcc-c++ cmake ninja-build libX11-devel libXcursor-devel libXi-devel libXrandr-devel mesa-libGL-devel fontconfig-devel libstdc++-devel".into()
    } else if family.contains("arch") {
        "Arch packages: clang gcc cmake ninja libx11 libxcursor libxi libxrandr mesa fontconfig"
            .into()
    } else if family.contains("suse") || family.contains("opensuse") {
        "SUSE packages: clang gcc-c++ cmake ninja libX11-devel libXcursor-devel libXi-devel libXrandr-devel Mesa-libGL-devel fontconfig-devel libstdc++-devel".into()
    } else {
        "Install Clang, the GNU C++ standard library toolchain, CMake, Ninja and the X11/Xcursor/Xi/Xrandr/OpenGL/fontconfig development packages with your distribution package manager.".into()
    }
}

fn steam_library_roots(home: &Path) -> BTreeSet<PathBuf> {
    let mut roots = BTreeSet::new();
    for root in [
        home.join(".steam/steam"),
        home.join(".local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ] {
        roots.insert(root.clone());
        let file = root.join("steamapps/libraryfolders.vdf");
        if let Ok(contents) = std::fs::read_to_string(file) {
            for path in parse_steam_library_paths(&contents) {
                roots.insert(path);
            }
        }
    }
    roots
}

fn parse_steam_library_paths(contents: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in contents.lines() {
        let quoted = line
            .split('"')
            .enumerate()
            .filter_map(|(index, part)| (index % 2 == 1).then_some(part))
            .collect::<Vec<_>>();
        if quoted.len() >= 2 && quoted[0].eq_ignore_ascii_case("path") {
            let path = PathBuf::from(quoted[1].replace("\\\\", "\\"));
            if path.is_absolute() {
                paths.push(path);
            }
        }
    }
    paths
}

fn infer_channel(path: &Path, fallback: InstallationChannel) -> InstallationChannel {
    let lowercase = path.to_string_lossy().to_ascii_lowercase();
    if lowercase.contains("/steamapps/") {
        InstallationChannel::Steam
    } else if lowercase.contains("/flatpak/")
        || lowercase.starts_with("/snap/")
        || lowercase.starts_with("/usr/")
        || lowercase.starts_with("/opt/")
    {
        InstallationChannel::PackageManager
    } else {
        fallback
    }
}

fn channel_rank(channel: &InstallationChannel) -> u8 {
    match channel {
        InstallationChannel::Managed => 0,
        InstallationChannel::Manual => 1,
        InstallationChannel::Steam => 2,
        InstallationChannel::PackageManager => 3,
    }
}

fn managed_fingerprint_matches(record: &crate::models::ManagedRecord) -> bool {
    let Some(expected) = record.bundle_fingerprint.as_deref() else {
        return false;
    };
    artifact_fingerprint(Path::new(&record.path))
        .map(|actual| hex::encode(actual).eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf())
        == std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf())
}

fn is_internal_installer_artifact(paths: &InstallerPaths, candidate: &Path) -> bool {
    let normalized = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    [&paths.cache_dir, &paths.backups_dir]
        .iter()
        .map(|root| std::fs::canonicalize(root).unwrap_or_else(|_| (*root).clone()))
        .any(|root| normalized.starts_with(root))
}

fn executable_regular_file(path: &Path) -> bool {
    let metadata = match path.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return false,
    };
    metadata.permissions().mode() & 0o111 != 0
}

fn trusted_desktop_opener(error_code: &str) -> AppResult<PathBuf> {
    ["/usr/bin/xdg-open", "/usr/bin/gio"]
        .iter()
        .map(PathBuf::from)
        .find(|candidate| executable_regular_file(candidate))
        .ok_or_else(|| {
            InstallerError::new(
                error_code,
                "No trusted xdg-open or gio desktop opener is installed.",
            )
        })
}

fn trusted_runtime_launcher(name: &str, error_code: &str) -> AppResult<PathBuf> {
    [
        PathBuf::from("/usr/bin").join(name),
        PathBuf::from("/bin").join(name),
    ]
    .into_iter()
    .find(|candidate| executable_regular_file(candidate))
    .and_then(|candidate| std::fs::canonicalize(candidate).ok())
    .filter(|candidate| validate_elf64_x86_64(candidate, true).is_ok())
    .ok_or_else(|| {
        InstallerError::with_detail(
            error_code,
            "The trusted package-manager launcher is unavailable.",
            name,
        )
    })
}

fn is_flatpak_aseprite_root(path: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    [
        home.join(format!(
            ".local/share/flatpak/app/{FLATPAK_ASEPRITE_ID}/current/active/files"
        )),
        PathBuf::from(format!(
            "/var/lib/flatpak/app/{FLATPAK_ASEPRITE_ID}/current/active/files"
        )),
    ]
    .into_iter()
    .any(|root| paths_equal(&root, path))
}

fn is_snap_aseprite_launcher(path: &Path) -> bool {
    path == Path::new("/snap/bin/aseprite")
}

fn is_writable_location(path: &Path) -> bool {
    let directory = path.parent().unwrap_or(path);
    let Ok(directory) = CString::new(directory.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(directory.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

fn strip_deleted_suffix(path: PathBuf) -> PathBuf {
    let value = path.as_os_str().as_bytes();
    let suffix = b" (deleted)";
    if value.ends_with(suffix) {
        PathBuf::from(OsStr::from_bytes(&value[..value.len() - suffix.len()]))
    } else {
        path
    }
}

fn find_png_icon(root: &Path) -> AppResult<PathBuf> {
    let icon_root = root.join("data/icons");
    let mut candidates = Vec::new();
    if icon_root.is_dir() {
        for entry in WalkDir::new(&icon_root)
            .max_depth(3)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            if !entry
                .path()
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
            {
                continue;
            }
            let mut header = [0_u8; 24];
            if open_regular_no_follow(entry.path(), "desktopIntegration")
                .and_then(|mut file| file.read_exact(&mut header).map_err(InstallerError::from))
                .is_err()
                || validate_png(&header).is_err()
            {
                continue;
            }
            let width = u32::from_be_bytes([header[16], header[17], header[18], header[19]]);
            let height = u32::from_be_bytes([header[20], header[21], header[22], header[23]]);
            candidates.push((width.saturating_mul(height), entry.path().to_path_buf()));
        }
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    candidates
        .into_iter()
        .next()
        .map(|(_, path)| path)
        .ok_or_else(|| {
            InstallerError::new(
                "desktopIntegration",
                "No valid PNG icon was found in the installed Aseprite data.",
            )
        })
}

fn validate_png(contents: &[u8]) -> Result<(), String> {
    if contents.len() < 24 || &contents[..8] != b"\x89PNG\r\n\x1a\n" || &contents[12..16] != b"IHDR"
    {
        return Err("The file does not contain a complete PNG IHDR header.".into());
    }
    let width = u32::from_be_bytes([contents[16], contents[17], contents[18], contents[19]]);
    let height = u32::from_be_bytes([contents[20], contents[21], contents[22], contents[23]]);
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(format!("The PNG dimensions are invalid: {width}x{height}."));
    }
    Ok(())
}

fn validate_integration_id(installation_id: &str) -> AppResult<()> {
    if installation_id.is_empty()
        || installation_id.len() > 128
        || !installation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(InstallerError::with_detail(
            "desktopIntegrationId",
            "The installation identifier cannot be used for desktop integration.",
            installation_id,
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct LinuxIntegrationLayout {
    data_root: PathBuf,
    desktop_path: PathBuf,
    icon_path: PathBuf,
}

fn validate_linux_integration_paths(
    paths: &[PathBuf],
    installation_id: &str,
    require_complete: bool,
    allow_create_root: bool,
) -> AppResult<LinuxIntegrationLayout> {
    validate_integration_id(installation_id)?;
    if (require_complete && paths.len() != 2) || paths.len() > 2 {
        return Err(InstallerError::with_detail(
            "desktopIntegrationPath",
            "The desktop integration path set is incomplete or contains unexpected entries.",
            format!("Expected two managed paths; received {}.", paths.len()),
        ));
    }

    let base_name = format!("{DESKTOP_PREFIX}{installation_id}");
    let desktop_name = OsString::from(format!("{base_name}.desktop"));
    let icon_name = OsString::from(format!("{base_name}.png"));
    let mut data_root = None::<PathBuf>;
    let mut saw_desktop = false;
    let mut saw_icon = false;
    let mut unique = BTreeSet::new();

    for path in paths {
        validate_absolute_normal_path(path)?;
        if !unique.insert(path.clone()) {
            return Err(InstallerError::with_detail(
                "desktopIntegrationPath",
                "The desktop integration path set contains a duplicate entry.",
                path.display().to_string(),
            ));
        }
        let (candidate_root, role) = if path.file_name() == Some(desktop_name.as_os_str())
            && path.parent().and_then(Path::file_name) == Some(OsStr::new("applications"))
        {
            let root = path.parent().and_then(Path::parent).ok_or_else(|| {
                InstallerError::with_detail(
                    "desktopIntegrationPath",
                    "The desktop launcher path has no per-user data root.",
                    path.display().to_string(),
                )
            })?;
            (root, 0_u8)
        } else if path.file_name() == Some(icon_name.as_os_str())
            && path.parent().and_then(Path::file_name) == Some(OsStr::new("icons"))
            && path
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                == Some(OsStr::new("aseprite-installer"))
        {
            let root = path
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .ok_or_else(|| {
                    InstallerError::with_detail(
                        "desktopIntegrationPath",
                        "The desktop icon path has no per-user data root.",
                        path.display().to_string(),
                    )
                })?;
            (root, 1_u8)
        } else {
            return Err(InstallerError::with_detail(
                "desktopIntegrationPath",
                "A desktop integration path does not have the deterministic managed name and location.",
                path.display().to_string(),
            ));
        };

        match role {
            0 if !saw_desktop => saw_desktop = true,
            1 if !saw_icon => saw_icon = true,
            _ => {
                return Err(InstallerError::with_detail(
                    "desktopIntegrationPath",
                    "The desktop integration path set contains the same managed role more than once.",
                    path.display().to_string(),
                ));
            }
        }
        if data_root
            .as_ref()
            .is_some_and(|existing| existing != candidate_root)
        {
            return Err(InstallerError::new(
                "desktopIntegrationPath",
                "The desktop launcher and icon do not share one per-user data root.",
            ));
        }
        data_root.get_or_insert_with(|| candidate_root.to_path_buf());
    }

    if require_complete && (!saw_desktop || !saw_icon) {
        return Err(InstallerError::new(
            "desktopIntegrationPath",
            "The desktop integration path set does not contain both managed roles.",
        ));
    }
    let data_root = data_root.ok_or_else(|| {
        InstallerError::new(
            "desktopIntegrationPath",
            "The desktop integration path set is empty.",
        )
    })?;
    validate_linux_data_root(&data_root, allow_create_root)?;

    let desktop_path = data_root
        .join("applications")
        .join(format!("{base_name}.desktop"));
    let icon_path = data_root
        .join("aseprite-installer/icons")
        .join(format!("{base_name}.png"));
    for path in paths {
        validate_linux_path_chain(&data_root, path, false)?;
    }
    Ok(LinuxIntegrationLayout {
        data_root,
        desktop_path,
        icon_path,
    })
}

fn validate_absolute_normal_path(path: &Path) -> AppResult<()> {
    if !path.is_absolute()
        || path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(InstallerError::with_detail(
            "desktopIntegrationPath",
            "A desktop integration path must be an absolute path without traversal components.",
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn validate_linux_data_root(data_root: &Path, allow_create: bool) -> AppResult<()> {
    validate_absolute_normal_path(data_root)?;
    let home = dirs::home_dir().ok_or_else(|| {
        InstallerError::new("home", "The current user home directory is unavailable.")
    })?;
    validate_absolute_normal_path(&home)?;
    let within_home = data_root.starts_with(&home);
    validate_unix_directory_chain(data_root, data_root)?;

    match std::fs::symlink_metadata(data_root) {
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o022 == 0 =>
        {
            Ok(())
        }
        Ok(_) => Err(InstallerError::with_detail(
            "desktopIntegrationDirectory",
            "The per-user data root is not a private real directory owned by the current user.",
            data_root.display().to_string(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_create && within_home => {
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !allow_create => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(
            InstallerError::with_detail(
                "desktopIntegrationDirectory",
                "A per-user data root outside the home directory must already exist and be privately owned.",
                data_root.display().to_string(),
            ),
        ),
        Err(error) => Err(error.into()),
    }
}

fn validate_unix_directory_chain(stop: &Path, user_owned_from: &Path) -> AppResult<()> {
    let mut current = PathBuf::new();
    for component in stop.components() {
        current.push(component.as_os_str());
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(InstallerError::with_detail(
                "desktopIntegrationDirectory",
                "A desktop integration directory chain contains a link or non-directory.",
                current.display().to_string(),
            ));
        }
        if metadata.mode() & 0o022 != 0 {
            return Err(InstallerError::with_detail(
                "desktopIntegrationDirectory",
                "A desktop integration directory chain is writable by another user.",
                current.display().to_string(),
            ));
        }
        if current.starts_with(user_owned_from) && metadata.uid() != unsafe { libc::geteuid() } {
            return Err(InstallerError::with_detail(
                "desktopIntegrationDirectory",
                "A desktop integration directory is not owned by the current user.",
                current.display().to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_linux_path_chain(data_root: &Path, path: &Path, include_leaf: bool) -> AppResult<()> {
    validate_absolute_normal_path(path)?;
    if !path.starts_with(data_root) {
        return Err(InstallerError::with_detail(
            "desktopIntegrationPath",
            "A desktop integration path escaped its per-user data root.",
            path.display().to_string(),
        ));
    }
    let stop = if include_leaf {
        path
    } else {
        path.parent().ok_or_else(|| {
            InstallerError::new(
                "desktopIntegrationPath",
                "A desktop integration path has no parent directory.",
            )
        })?
    };
    validate_unix_directory_chain(stop, data_root)
}

fn ensure_integration_directory(data_root: &Path, directory: &Path) -> AppResult<()> {
    validate_linux_path_chain(data_root, directory, true)?;
    let mut current = PathBuf::new();
    for component in directory.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata)
                if metadata.is_dir()
                    && !metadata.file_type().is_symlink()
                    && metadata.mode() & 0o022 == 0
                    && (!current.starts_with(data_root)
                        || metadata.uid() == unsafe { libc::geteuid() }) => {}
            Ok(_) => {
                return Err(InstallerError::with_detail(
                    "desktopIntegrationDirectory",
                    "A desktop integration directory is not a private real directory.",
                    current.display().to_string(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)?;
                let metadata = std::fs::symlink_metadata(&current)?;
                if !metadata.is_dir()
                    || metadata.file_type().is_symlink()
                    || metadata.uid() != unsafe { libc::geteuid() }
                    || metadata.mode() & 0o022 != 0
                {
                    return Err(InstallerError::with_detail(
                        "desktopIntegrationDirectory",
                        "A newly created desktop integration directory could not be secured.",
                        current.display().to_string(),
                    ));
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    validate_linux_path_chain(data_root, directory, true)?;
    Ok(())
}

fn atomic_write(path: &Path, contents: &[u8], mode: u32) -> AppResult<()> {
    let parent = path.parent().ok_or_else(|| {
        InstallerError::new("atomicWrite", "The destination has no parent directory.")
    })?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(InstallerError::with_detail(
                "atomicWriteType",
                "An existing destination is not a regular file.",
                path.display().to_string(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let temporary = parent.join(format!(".aseprite-installer-write-{}", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(mode)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn desktop_exec_argument(path: &Path) -> AppResult<String> {
    let value = path.to_str().ok_or_else(|| {
        InstallerError::new(
            "desktopIntegrationPath",
            "The Aseprite executable path is not valid UTF-8.",
        )
    })?;
    if value.contains(['\n', '\r']) {
        return Err(InstallerError::new(
            "desktopIntegrationPath",
            "The Aseprite executable path contains a line break.",
        ));
    }
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%");
    Ok(format!("\"{escaped}\""))
}

fn desktop_plain_path(path: &Path) -> AppResult<String> {
    let value = path.to_str().ok_or_else(|| {
        InstallerError::new(
            "desktopIntegrationPath",
            "A desktop integration path is not valid UTF-8.",
        )
    })?;
    if value.contains(['\n', '\r']) {
        return Err(InstallerError::new(
            "desktopIntegrationPath",
            "A desktop integration path contains a line break.",
        ));
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    fn write_elf(path: &Path, machine: u16) {
        let mut header = [0_u8; 64];
        header[..4].copy_from_slice(b"\x7fELF");
        header[4] = 2;
        header[5] = 1;
        header[6] = 1;
        header[16..18].copy_from_slice(&2_u16.to_le_bytes());
        header[18..20].copy_from_slice(&machine.to_le_bytes());
        header[20..24].copy_from_slice(&1_u32.to_le_bytes());
        std::fs::write(path, header).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn parses_versions_without_accepting_partial_components() {
        assert_eq!(
            parse_numeric_version("Ubuntu clang version 18.1.3"),
            Some([18, 1, 3])
        );
        assert_eq!(
            extract_version("Aseprite v1.3.18.1-dev"),
            Some("1.3.18.1".into())
        );
        assert_eq!(extract_version("version unknown"), None);
    }

    #[test]
    fn elf_parser_requires_x86_64() {
        let directory = tempdir().unwrap();
        let valid = directory.path().join("valid");
        let arm = directory.path().join("arm");
        write_elf(&valid, 62);
        write_elf(&arm, 183);
        assert!(validate_elf64_x86_64(&valid, true).is_ok());
        assert!(validate_elf64_x86_64(&arm, true).is_err());
    }

    #[test]
    fn fingerprint_is_stable_and_rejects_links() {
        let directory = tempdir().unwrap();
        std::fs::create_dir(directory.path().join("data")).unwrap();
        std::fs::write(directory.path().join("data/file"), b"content").unwrap();
        let first = artifact_fingerprint(directory.path()).unwrap();
        let second = artifact_fingerprint(directory.path()).unwrap();
        assert_eq!(first, second);
        symlink("file", directory.path().join("data/link")).unwrap();
        assert!(artifact_fingerprint(directory.path()).is_err());
    }

    #[test]
    fn desktop_entry_ownership_requires_the_exact_managed_id() {
        let directory = tempdir().unwrap();
        let desktop = directory.path().join("managed.desktop");
        std::fs::write(
            &desktop,
            format!(
                "[Desktop Entry]\nX-Aseprite-Installer-Id=aseprite-owned\nX-Aseprite-Installer-Icon-Sha256={}\n",
                "a".repeat(64)
            ),
        )
        .unwrap();

        assert!(desktop_entry_is_owned(&desktop, "aseprite-owned").unwrap());
        assert!(!desktop_entry_is_owned(&desktop, "aseprite-other").unwrap());
    }

    #[test]
    fn finds_only_complete_build_layouts() {
        let directory = tempdir().unwrap();
        let bin = directory.path().join("build/bin");
        std::fs::create_dir_all(bin.join("data")).unwrap();
        write_elf(&bin.join("aseprite"), 62);
        std::fs::write(bin.join("data/gui.xml"), b"gui").unwrap();
        std::fs::write(bin.join("data/pref.xml"), b"pref").unwrap();
        assert_eq!(
            find_built_artifact(&directory.path().join("build")).unwrap(),
            bin
        );
    }

    #[test]
    fn parses_only_absolute_steam_library_paths() {
        let paths = parse_steam_library_paths(
            r#""path" "/mnt/Games"
"path" "relative/Games"
"path" "D:\\Steam""#,
        );
        assert_eq!(paths, vec![PathBuf::from("/mnt/Games")]);
    }

    #[test]
    fn desktop_discovery_accepts_only_direct_absolute_aseprite_exec_paths() {
        let entry = r#"[Desktop Entry]
Type=Application
Name=Aseprite
Exec="/opt/My Aseprite/Aseprite.AppImage" %F
"#;
        assert_eq!(
            parse_desktop_entry_executable(entry, "portable.desktop"),
            Some(PathBuf::from("/opt/My Aseprite/Aseprite.AppImage"))
        );

        let hidden = entry.replace("Name=Aseprite", "Name=Aseprite\nHidden=true");
        assert_eq!(
            parse_desktop_entry_executable(&hidden, "aseprite.desktop"),
            None
        );
        assert_eq!(
            parse_desktop_entry_executable(
                "[Desktop Entry]\nType=Application\nName=Aseprite\nExec=env FOO=bar /opt/Aseprite\n",
                "aseprite.desktop"
            ),
            None
        );
        assert_eq!(
            parse_desktop_entry_executable(
                "[Desktop Entry]\nType=Application\nName=Aseprite\nExec=/usr/bin/flatpak run org.aseprite.Aseprite\n",
                "org.aseprite.Aseprite.desktop"
            ),
            None
        );
        assert_eq!(
            parse_desktop_entry_executable(
                "[Desktop Entry]\nType=Application\nName=Aseprite\nTryExec=/opt/Aseprite.AppImage\nExec=/usr/bin/flatpak run org.aseprite.Aseprite\n",
                "org.aseprite.Aseprite.desktop"
            ),
            None
        );
        assert_eq!(
            parse_desktop_entry_executable(
                "[Desktop Entry]\nType=Application\nName=Other\nExec=/opt/Aseprite\n",
                "other.desktop"
            ),
            None
        );
    }

    #[test]
    fn launch_validation_resolves_a_discovered_symlink_before_no_follow_inspection() {
        let directory = tempdir().unwrap();
        let executable = directory.path().join("aseprite-real");
        let link = directory.path().join("aseprite");
        write_elf(&executable, 62);
        symlink(&executable, &link).unwrap();

        assert_eq!(
            validated_launch_executable(&link).unwrap(),
            std::fs::canonicalize(executable).unwrap()
        );
    }

    #[test]
    fn desktop_exec_escapes_field_codes_and_shell_metacharacters() {
        let escaped = desktop_exec_argument(Path::new("/tmp/A $HOME/%game/aseprite")).unwrap();
        assert_eq!(escaped, "\"/tmp/A \\$HOME/%%game/aseprite\"");
    }

    #[test]
    fn cmake_arguments_pin_direct_tools_and_libstdcxx() {
        let environment = BuildEnvironment {
            path: OsString::from(SYSTEM_PATH),
            clang: PathBuf::from("/usr/bin/clang"),
            clangxx: PathBuf::from("/usr/bin/clang++"),
            cmake: PathBuf::from("/usr/bin/cmake"),
            ninja: PathBuf::from("/usr/bin/ninja"),
            home_dir: PathBuf::from("/nonexistent"),
            temporary_dir: PathBuf::from("/tmp"),
        };
        let arguments = cmake_arguments(
            Path::new("/source"),
            Path::new("/build"),
            Path::new("/skia"),
            &environment,
        );
        assert!(arguments.contains(&OsString::from(
            "-DCMAKE_CXX_FLAGS:STRING=-stdlib=libstdc++"
        )));
        assert!(arguments.contains(&OsString::from(
            "-DCMAKE_MAKE_PROGRAM:FILEPATH=/usr/bin/ninja"
        )));
        assert!(arguments.contains(&OsString::from("-DLAF_BACKEND:STRING=skia")));
    }

    #[test]
    fn os_release_parser_handles_quoted_values() {
        let values =
            parse_os_release("ID=ubuntu\nID_LIKE=debian\nPRETTY_NAME=\"Ubuntu 24.04 LTS\"\n");
        assert_eq!(values.get("ID").map(String::as_str), Some("ubuntu"));
        assert_eq!(
            values.get("PRETTY_NAME").map(String::as_str),
            Some("Ubuntu 24.04 LTS")
        );
    }

    #[test]
    fn distro_hints_cover_the_probed_gnu_cxx_and_xrandr_dependencies() {
        for (id, id_like, compiler_package, xrandr_package) in [
            ("ubuntu", "debian", "g++", "libxrandr-dev"),
            ("fedora", "", "gcc-c++", "libXrandr-devel"),
            ("arch", "", "gcc", "libxrandr"),
            ("opensuse", "suse", "gcc-c++", "libXrandr-devel"),
        ] {
            let hint = distro_install_hint(&LinuxDistribution {
                id: id.into(),
                id_like: id_like.into(),
                pretty_name: id.into(),
            });
            assert!(hint
                .split_whitespace()
                .any(|package| package == compiler_package));
            assert!(hint
                .split_whitespace()
                .any(|package| package == xrandr_package));
            assert!(!hint.contains("libc++"));
        }
    }
}
