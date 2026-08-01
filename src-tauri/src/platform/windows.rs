use crate::error::{AppResult, InstallerError};
use crate::models::{
    InstallationChannel, InstallationInfo, ManagedState, PreflightReport, Prerequisite,
};
use crate::platform::{PlatformAdapter, PreflightContext};
use crate::portable_transaction::{
    durable_rename_file_no_replace, IntegrationSnapshot, MAX_INTEGRATION_FILE_BYTES,
};
use crate::state::InstallerPaths;
use async_trait::async_trait;
use fs2::{available_space, FileExt};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{c_void, OsStr, OsString};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use uuid::Uuid;
use walkdir::WalkDir;
use windows_sys::Win32::Foundation::{
    ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_EXPAND_SZ, REG_SZ,
};
use windows_sys::Win32::System::Threading::{
    OpenThread, ResumeThread, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
};

const BUILD_SAFETY_BUDGET_BYTES: u64 = 6 * 1024 * 1024 * 1024;
const WINDOWS_11_MINIMUM_BUILD: u64 = 22_000;
const WINDOWS_SDK_VERSION: &str = "10.0.26100.0";
const COMMAND_OUTPUT_LIMIT: usize = 256 * 1024;
const MAX_REGISTRY_STRING_BYTES: u32 = 64 * 1024;
const MAX_UNINSTALL_SUBKEYS: u32 = 2_048;
const MAX_CHOCOLATEY_ENTRIES: usize = 1_024;
// A Windows environment block is normally far smaller than this. Keeping a
// separate, larger cap avoids truncating a Visual Studio developer environment
// while still placing a strict bound on untrusted command output.
const VC_ENVIRONMENT_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
const FILE_ADD_SUBDIRECTORY: u32 = 0x0000_0004;
const DELETE_ACCESS: u32 = 0x0001_0000;
const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const FILE_SHARE_DELETE: u32 = 0x0000_0004;
const SHORTCUT_PREFIX: &str = "Aseprite (Local Build - ";
const SHORTCUT_SUFFIX: &str = ").lnk";
const SHORTCUT_DESCRIPTION_PREFIX: &str =
    "Personal Aseprite source build managed by Aseprite Installer (";

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const FOLDER_ID_PROFILE: Guid = Guid {
    data1: 0x5e6c_858f,
    data2: 0x0e22,
    data3: 0x4760,
    data4: [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
};
const FOLDER_ID_ROAMING_APP_DATA: Guid = Guid {
    data1: 0x3eb6_85db,
    data2: 0x65f9,
    data3: 0x4cf6,
    data4: [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d],
};

#[repr(C)]
#[derive(Clone, Copy)]
struct FileTime {
    low: u32,
    high: u32,
}

#[repr(C)]
struct ByHandleFileInformation {
    attributes: u32,
    creation_time: FileTime,
    last_access_time: FileTime,
    last_write_time: FileTime,
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn ExpandEnvironmentStringsW(source: *const u16, destination: *mut u16, size: u32) -> u32;
    fn GetFileInformationByHandle(
        file: *mut c_void,
        information: *mut ByHandleFileInformation,
    ) -> i32;
    fn GetWindowsDirectoryW(buffer: *mut u16, size: u32) -> u32;
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

#[link(name = "shell32")]
extern "system" {
    fn SHGetKnownFolderPath(
        folder_id: *const Guid,
        flags: u32,
        token: *mut c_void,
        path: *mut *mut u16,
    ) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoTaskMemFree(memory: *const c_void);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    volume_serial_number: u32,
    file_index: u64,
}

#[derive(Debug, Clone, Copy)]
struct FileHandleInformation {
    identity: FileIdentity,
    attributes: u32,
    size: u64,
    last_write_time: u64,
}

#[derive(Debug)]
struct VolumeBudget {
    volume: String,
    roles: Vec<&'static str>,
    available_bytes: u64,
}

#[derive(Debug)]
struct DiskSpaceBudget {
    volumes: Vec<VolumeBudget>,
}

impl DiskSpaceBudget {
    fn ready(&self) -> bool {
        self.volumes
            .iter()
            .all(|volume| volume.available_bytes >= BUILD_SAFETY_BUDGET_BYTES)
    }

    fn minimum_available(&self) -> u64 {
        self.volumes
            .iter()
            .map(|volume| volume.available_bytes)
            .min()
            .unwrap_or(0)
    }

    fn detail(&self) -> String {
        self.volumes
            .iter()
            .map(|volume| {
                format!(
                    "{} ({}): {} bytes available",
                    volume.volume,
                    volume.roles.join(", "),
                    volume.available_bytes
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[derive(Debug, Clone)]
pub struct BuildEnvironment {
    pub path: OsString,
    pub cl: PathBuf,
    pub cmake: PathBuf,
    pub ninja: PathBuf,
    environment: BTreeMap<OsString, OsString>,
}

/// Kernel-owned process tree used for every Windows compiler or probe command.
/// Child processes inherit job membership, and closing the last handle kills
/// any descendants that outlive the direct process or keep output pipes open.
#[derive(Debug)]
pub struct ProcessTreeJob {
    handle: OwnedHandle,
}

impl ProcessTreeJob {
    pub fn new() -> AppResult<Self> {
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(InstallerError::with_detail(
                "processJob",
                "Windows could not create an isolated compiler process job.",
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw as RawHandle) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle.as_raw_handle() as _,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(InstallerError::with_detail(
                "processJob",
                "Windows could not configure safe compiler process cleanup.",
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(Self { handle })
    }

    pub fn prepare_tokio_command(command: &mut tokio::process::Command) {
        command.creation_flags(CREATE_SUSPENDED);
    }

    fn prepare_std_command(command: &mut std::process::Command) {
        command.creation_flags(CREATE_SUSPENDED);
    }

    pub fn assign_and_resume_tokio_child(&self, child: &tokio::process::Child) -> AppResult<()> {
        let handle = child.raw_handle().ok_or_else(|| {
            InstallerError::new(
                "processJob",
                "The Windows compiler process handle is unavailable.",
            )
        })?;
        self.assign_raw_handle(handle)?;
        let process_id = child.id().ok_or_else(|| {
            InstallerError::new(
                "processJob",
                "The suspended Windows compiler process identifier is unavailable.",
            )
        })?;
        resume_primary_thread(process_id)
    }

    fn assign_and_resume_std_child(&self, child: &std::process::Child) -> AppResult<()> {
        self.assign_raw_handle(child.as_raw_handle())?;
        resume_primary_thread(child.id())
    }

    fn assign_raw_handle(&self, process: RawHandle) -> AppResult<()> {
        let assigned =
            unsafe { AssignProcessToJobObject(self.handle.as_raw_handle() as _, process as _) };
        if assigned == 0 {
            Err(InstallerError::with_detail(
                "processJob",
                "Windows could not attach a compiler process to its cleanup job.",
                std::io::Error::last_os_error().to_string(),
            ))
        } else {
            Ok(())
        }
    }

    pub fn terminate(&self) -> AppResult<()> {
        let terminated = unsafe { TerminateJobObject(self.handle.as_raw_handle() as _, 1) };
        if terminated == 0 {
            Err(InstallerError::with_detail(
                "processJob",
                "Windows could not terminate the complete compiler process tree.",
                std::io::Error::last_os_error().to_string(),
            ))
        } else {
            Ok(())
        }
    }
}

fn resume_primary_thread(process_id: u32) -> AppResult<()> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(InstallerError::with_detail(
            "processJob",
            "Windows could not enumerate the suspended compiler process thread.",
            std::io::Error::last_os_error().to_string(),
        ));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot as RawHandle) };
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..THREADENTRY32::default()
    };
    let mut available = unsafe { Thread32First(snapshot.as_raw_handle() as _, &mut entry) } != 0;
    while available {
        if entry.th32OwnerProcessID == process_id {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                return Err(InstallerError::with_detail(
                    "processJob",
                    "Windows could not open the suspended compiler process thread.",
                    std::io::Error::last_os_error().to_string(),
                ));
            }
            let thread = unsafe { OwnedHandle::from_raw_handle(thread as RawHandle) };
            let previous_count = unsafe { ResumeThread(thread.as_raw_handle() as _) };
            if previous_count == u32::MAX {
                return Err(InstallerError::with_detail(
                    "processJob",
                    "Windows could not resume the compiler process after assigning its job.",
                    std::io::Error::last_os_error().to_string(),
                ));
            }
            return Ok(());
        }
        available = unsafe { Thread32Next(snapshot.as_raw_handle() as _, &mut entry) } != 0;
    }
    Err(InstallerError::with_detail(
        "processJob",
        "Windows could not find the suspended compiler process primary thread.",
        process_id.to_string(),
    ))
}

impl BuildEnvironment {
    pub fn configure(&self, command: &mut tokio::process::Command) {
        command.env_clear();
        command.envs(self.environment.iter());
        command.env("PATH", &self.path);
    }

    fn configure_std(&self, command: &mut Command) {
        command.env_clear();
        command.envs(self.environment.iter());
        command.env("PATH", &self.path);
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsAdapter;

impl WindowsAdapter {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PlatformAdapter for WindowsAdapter {
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
                    "Aseprite installations could not be scanned on Windows.",
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
                    "The Windows build environment could not be checked.",
                    error.to_string(),
                )
            })?
    }

    fn default_target(&self) -> AppResult<PathBuf> {
        let local_app_data = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
            InstallerError::new(
                "userProfile",
                "Windows did not provide the current user's Local AppData directory.",
            )
        })?;
        Ok(PathBuf::from(local_app_data)
            .join("Programs")
            .join("Aseprite"))
    }
}

fn discover(paths: &InstallerPaths, managed: &ManagedState) -> AppResult<Vec<InstallationInfo>> {
    let mut candidates = BTreeMap::<PathBuf, InstallationChannel>::new();
    for record in &managed.installations {
        candidates.insert(PathBuf::from(&record.path), InstallationChannel::Managed);
    }
    if let Ok(default) = WindowsAdapter::new().default_target() {
        candidates
            .entry(default)
            .or_insert(InstallationChannel::Manual);
    }
    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(value) = std::env::var_os(variable) {
            let value = PathBuf::from(value);
            if !value.is_absolute() || path_is_unc(&value) {
                continue;
            }
            candidates
                .entry(value.join("Aseprite"))
                .or_insert(InstallationChannel::Manual);
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        if local.is_absolute() && !path_is_unc(&local) {
            candidates
                .entry(local.join("Programs").join("Aseprite"))
                .or_insert(InstallationChannel::Manual);
            candidates
                .entry(local.join("Microsoft").join("WinGet").join("Packages"))
                .or_insert(InstallationChannel::PackageManager);
        }
    }
    if let Some(home) = dirs::home_dir() {
        if home.is_absolute() && !path_is_unc(&home) {
            candidates
                .entry(
                    home.join("scoop")
                        .join("apps")
                        .join("aseprite")
                        .join("current"),
                )
                .or_insert(InstallationChannel::PackageManager);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            if !directory.is_absolute() || path_is_unc(&directory) {
                continue;
            }
            let executable = directory.join("aseprite.exe");
            if executable.is_file() {
                candidates
                    .entry(directory)
                    .or_insert(InstallationChannel::Manual);
            }
        }
    }
    for candidate in registry_aseprite_candidates() {
        candidates
            .entry(candidate)
            .or_insert(InstallationChannel::Manual);
    }
    for candidate in chocolatey_aseprite_candidates() {
        candidates
            .entry(candidate)
            .or_insert(InstallationChannel::PackageManager);
    }
    for library in steam_library_roots() {
        candidates
            .entry(library.join("steamapps").join("common").join("Aseprite"))
            .or_insert(InstallationChannel::Steam);
    }

    let mut installations = Vec::new();
    let mut seen = BTreeSet::new();
    for (candidate, inferred_channel) in candidates {
        if candidate
            .file_name()
            .is_some_and(|name| name == OsStr::new("Packages"))
        {
            collect_named_children(
                &candidate,
                inferred_channel,
                &mut installations,
                &mut seen,
                managed,
            );
            continue;
        }
        if inferred_channel != InstallationChannel::Managed
            && is_internal_installer_artifact(paths, &candidate)
        {
            continue;
        }
        if let Some(installation) = inspect_candidate(&candidate, inferred_channel, managed) {
            let key = installation.path.to_ascii_lowercase();
            if seen.insert(key) {
                installations.push(installation);
            }
        }
    }
    Ok(installations)
}

fn registry_aseprite_candidates() -> BTreeSet<PathBuf> {
    const APP_PATHS: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths\aseprite.exe";
    const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    let mut candidates = BTreeSet::new();
    for (root, view) in registry_roots_and_views() {
        if let Some(key) = open_registry_key(root, APP_PATHS, view) {
            if let Some(value) = query_registry_string(&key, None) {
                if let Some(path) = registry_local_path(&value, true) {
                    candidates.insert(path);
                }
            }
        }
        let Some(uninstall) = open_registry_key(root, UNINSTALL, view) else {
            continue;
        };
        for index in 0..MAX_UNINSTALL_SUBKEYS {
            let Some(name) = enum_registry_subkey(&uninstall, index) else {
                break;
            };
            let Some(entry) = open_registry_key(uninstall.0, &name, 0) else {
                continue;
            };
            let Some(display_name) = query_registry_string(&entry, Some("DisplayName")) else {
                continue;
            };
            if !display_name.to_ascii_lowercase().contains("aseprite") {
                continue;
            }
            if let Some(location) = query_registry_string(&entry, Some("InstallLocation"))
                .and_then(|value| registry_local_path(&value, false))
            {
                candidates.insert(location);
            }
            if let Some(icon) = query_registry_string(&entry, Some("DisplayIcon"))
                .and_then(|value| registry_local_path(&value, true))
            {
                candidates.insert(icon);
            }
        }
    }
    candidates
}

fn registry_steam_roots() -> BTreeSet<PathBuf> {
    const STEAM: &str = r"Software\Valve\Steam";
    let mut roots = BTreeSet::new();
    for (root, view) in registry_roots_and_views() {
        let Some(key) = open_registry_key(root, STEAM, view) else {
            continue;
        };
        for value_name in ["SteamPath", "InstallPath"] {
            if let Some(path) = query_registry_string(&key, Some(value_name))
                .and_then(|value| registry_local_path(&value, false))
            {
                roots.insert(path);
            }
        }
        if let Some(executable) = query_registry_string(&key, Some("SteamExe"))
            .and_then(|value| registry_local_path(&value, true))
        {
            if let Some(parent) = executable.parent() {
                roots.insert(parent.to_path_buf());
            }
        }
    }
    roots
}

fn registry_roots_and_views() -> [(HKEY, u32); 4] {
    [
        (HKEY_CURRENT_USER, KEY_WOW64_64KEY),
        (HKEY_CURRENT_USER, KEY_WOW64_32KEY),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
    ]
}

fn open_registry_key(root: HKEY, subkey: &str, view: u32) -> Option<RegistryKey> {
    let subkey = wide_null(subkey)?;
    let mut handle = std::ptr::null_mut();
    let status = unsafe { RegOpenKeyExW(root, subkey.as_ptr(), 0, KEY_READ | view, &mut handle) };
    (status == ERROR_SUCCESS && !handle.is_null()).then_some(RegistryKey(handle))
}

fn enum_registry_subkey(key: &RegistryKey, index: u32) -> Option<String> {
    let mut buffer = [0_u16; 512];
    let mut length = buffer.len() as u32;
    let status = unsafe {
        RegEnumKeyExW(
            key.0,
            index,
            buffer.as_mut_ptr(),
            &mut length,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if status == ERROR_NO_MORE_ITEMS {
        return None;
    }
    if status != ERROR_SUCCESS || length as usize > buffer.len() {
        return None;
    }
    String::from_utf16(&buffer[..length as usize]).ok()
}

fn query_registry_string(key: &RegistryKey, value_name: Option<&str>) -> Option<String> {
    let value_name = match value_name {
        Some(value) => Some(wide_null(value)?),
        None => None,
    };
    let name = value_name
        .as_ref()
        .map_or(std::ptr::null(), |value| value.as_ptr());
    for _ in 0..2 {
        let mut value_type = 0_u32;
        let mut byte_count = 0_u32;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                name,
                std::ptr::null(),
                &mut value_type,
                std::ptr::null_mut(),
                &mut byte_count,
            )
        };
        if status != ERROR_SUCCESS
            || !matches!(value_type, REG_SZ | REG_EXPAND_SZ)
            || byte_count == 0
            || byte_count > MAX_REGISTRY_STRING_BYTES
            || !byte_count.is_multiple_of(2)
        {
            return None;
        }
        let mut buffer = vec![0_u16; byte_count as usize / 2 + 1];
        let mut actual_count = byte_count;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                name,
                std::ptr::null(),
                &mut value_type,
                buffer.as_mut_ptr().cast(),
                &mut actual_count,
            )
        };
        if status == ERROR_MORE_DATA {
            continue;
        }
        if status != ERROR_SUCCESS
            || actual_count > byte_count
            || !actual_count.is_multiple_of(2)
            || !matches!(value_type, REG_SZ | REG_EXPAND_SZ)
        {
            return None;
        }
        buffer.truncate(actual_count as usize / 2);
        while buffer.last() == Some(&0) {
            buffer.pop();
        }
        if buffer.is_empty() || buffer.contains(&0) {
            return None;
        }
        let value = String::from_utf16(&buffer).ok()?;
        return if value_type == REG_EXPAND_SZ {
            expand_environment_string(&value)
        } else {
            Some(value)
        };
    }
    None
}

fn expand_environment_string(value: &str) -> Option<String> {
    let source = wide_null(value)?;
    let required = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), std::ptr::null_mut(), 0) };
    let maximum_units = MAX_REGISTRY_STRING_BYTES / 2;
    if required == 0 || required > maximum_units {
        return None;
    }
    let mut buffer = vec![0_u16; required as usize];
    let written = unsafe {
        ExpandEnvironmentStringsW(source.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32)
    };
    if written == 0 || written > buffer.len() as u32 {
        return None;
    }
    buffer.truncate(written.saturating_sub(1) as usize);
    let expanded = String::from_utf16(&buffer).ok()?;
    (!expanded.contains('%')).then_some(expanded)
}

fn wide_null(value: &str) -> Option<Vec<u16>> {
    (!value.contains('\0')).then(|| value.encode_utf16().chain(Some(0)).collect())
}

fn registry_local_path(value: &str, allow_icon_index: bool) -> Option<PathBuf> {
    let mut value = value.trim();
    if value.starts_with('"') {
        let closing = value[1..].find('"')? + 1;
        let suffix = value[closing + 1..].trim();
        if !suffix.is_empty()
            && !(allow_icon_index
                && suffix
                    .strip_prefix(',')
                    .is_some_and(|index| index.trim().parse::<i32>().is_ok()))
        {
            return None;
        }
        value = &value[1..closing];
    } else if allow_icon_index {
        if let Some((path, index)) = value.rsplit_once(',') {
            if index.trim().parse::<i32>().is_ok() {
                value = path.trim();
            }
        }
    }
    let path = PathBuf::from(value);
    if !path.is_absolute()
        || path_is_unc(&path)
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return None;
    }
    Some(path)
}

fn chocolatey_aseprite_candidates() -> BTreeSet<PathBuf> {
    let mut roots = BTreeSet::new();
    if let Some(root) = std::env::var_os("ChocolateyInstall") {
        let root = PathBuf::from(root);
        if root.is_absolute() && !path_is_unc(&root) {
            roots.insert(root);
        }
    }
    if let Some(program_data) = std::env::var_os("ProgramData") {
        let program_data = PathBuf::from(program_data);
        if program_data.is_absolute() && !path_is_unc(&program_data) {
            roots.insert(program_data.join("chocolatey"));
        }
    }
    let mut candidates = BTreeSet::new();
    let mut inspected = 0_usize;
    'roots: for root in roots {
        let packages = root.join("lib");
        let Ok(entries) = std::fs::read_dir(packages) else {
            continue;
        };
        for entry in entries.flatten() {
            if inspected >= MAX_CHOCOLATEY_ENTRIES {
                break 'roots;
            }
            inspected += 1;
            if !entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("aseprite")
            {
                continue;
            }
            for artifact in WalkDir::new(entry.path())
                .max_depth(6)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
            {
                if inspected >= MAX_CHOCOLATEY_ENTRIES {
                    break 'roots;
                }
                inspected += 1;
                if artifact.file_type().is_file()
                    && artifact
                        .file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case("aseprite.exe")
                {
                    if let Some(parent) = artifact.path().parent() {
                        candidates.insert(parent.to_path_buf());
                    }
                }
            }
        }
    }
    candidates
}

fn steam_library_roots() -> BTreeSet<PathBuf> {
    let mut roots = BTreeSet::new();
    for variable in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(program_files) = std::env::var_os(variable) {
            let program_files = PathBuf::from(program_files);
            if !program_files.is_absolute() || path_is_unc(&program_files) {
                continue;
            }
            let steam = program_files.join("Steam");
            if steam.is_dir() {
                roots.insert(steam);
            }
        }
    }
    roots.extend(registry_steam_roots());
    let manifests = roots
        .iter()
        .map(|root| root.join("steamapps/libraryfolders.vdf"))
        .collect::<Vec<_>>();
    for manifest in manifests {
        let Ok(metadata) = manifest.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > 1024 * 1024 {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        for path in parse_steam_library_paths(&contents).into_iter().take(128) {
            roots.insert(path);
        }
    }
    roots
}

fn parse_steam_library_paths(contents: &str) -> Vec<PathBuf> {
    contents
        .lines()
        .filter_map(|line| {
            let mut quoted = line.split('"');
            let _before = quoted.next()?;
            let key = quoted.next()?;
            let _separator = quoted.next()?;
            let value = quoted.next()?;
            key.eq_ignore_ascii_case("path").then_some(value)
        })
        .map(|value| PathBuf::from(value.replace("\\\\", "\\")))
        .filter(|path| path.is_absolute() && !path_is_unc(path))
        .collect()
}

fn collect_named_children(
    root: &Path,
    inferred_channel: InstallationChannel,
    installations: &mut Vec<InstallationInfo>,
    seen: &mut BTreeSet<String>,
    managed: &ManagedState,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten().take(256) {
        let path = entry.path();
        if !entry
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("aseprite")
        {
            continue;
        }
        if let Some(installation) = inspect_candidate(&path, inferred_channel.clone(), managed) {
            let key = installation.path.to_ascii_lowercase();
            if seen.insert(key) {
                installations.push(installation);
            }
        }
    }
}

fn inspect_candidate(
    candidate: &Path,
    inferred_channel: InstallationChannel,
    managed: &ManagedState,
) -> Option<InstallationInfo> {
    let root = if candidate.is_file() {
        candidate.parent()?.to_path_buf()
    } else {
        candidate.to_path_buf()
    };
    let executable = root.join("aseprite.exe");
    if !is_x64_pe(&executable).ok()? {
        return None;
    }
    let normalized = std::fs::canonicalize(&root).unwrap_or(root);
    let normalized_string = normalized.to_string_lossy().into_owned();
    let recorded = managed
        .installations
        .iter()
        .find(|record| paths_equal(Path::new(&record.path), &normalized));
    // Discovery is read-only: never execute a candidate supplied by PATH, the
    // registry, or another package manager. A persisted record is considered
    // managed only while its complete tree still matches the recorded digest.
    let managed_record = recorded.filter(|record| managed_fingerprint_matches(record));
    let channel = if managed_record.is_some() {
        InstallationChannel::Managed
    } else {
        infer_channel(&normalized, inferred_channel)
    };
    let writable = directory_is_probably_writable(&normalized);
    let manageable = managed_record.is_some()
        || (channel == InstallationChannel::Manual
            && writable
            && validate_artifact_structure(&normalized).is_ok()
            && artifact_fingerprint(&normalized).is_ok());
    let visible_path = managed_record
        .map(|record| record.path.clone())
        .unwrap_or(normalized_string);
    Some(InstallationInfo {
        id: managed_record
            .map(|record| record.id.clone())
            .unwrap_or_else(|| installation_id(&visible_path)),
        path: visible_path,
        version: managed_record
            .and_then(|record| record.source_version.clone())
            .or_else(|| managed_record.map(|record| record.tag.clone())),
        version_exact: managed_record.is_some_and(|record| record.version_exact),
        architecture: Some("x86_64".into()),
        channel,
        manageable,
        writable,
        has_backup: managed_record
            .and_then(|record| record.backup_path.as_deref())
            .is_some_and(|path| Path::new(path).exists()),
        installed_at: managed_record.map(|record| record.installed_at.clone()),
    })
}

fn infer_channel(path: &Path, fallback: InstallationChannel) -> InstallationChannel {
    let value = path.to_string_lossy().to_ascii_lowercase();
    if value.contains("steamapps") {
        InstallationChannel::Steam
    } else if value.contains("\\scoop\\")
        || value.contains("\\chocolatey\\")
        || value.contains("\\winget\\")
        || value.contains("\\program files\\windowsapps\\")
        || path_is_in_program_files(path)
    {
        InstallationChannel::PackageManager
    } else {
        fallback
    }
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    let left = std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

fn is_internal_installer_artifact(paths: &InstallerPaths, candidate: &Path) -> bool {
    let candidate = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    [&paths.cache_dir, &paths.backups_dir]
        .into_iter()
        .map(|root| std::fs::canonicalize(root).unwrap_or_else(|_| root.clone()))
        .any(|root| windows_path_is_within(&candidate, &root))
}

fn path_is_in_program_files(path: &Path) -> bool {
    ["ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .map(|root| std::fs::canonicalize(&root).unwrap_or(root))
        .any(|root| windows_path_is_within(path, &root))
}

fn directory_is_probably_writable(path: &Path) -> bool {
    if path_is_unc(path) || path_is_in_program_files(path) || !path.is_dir() {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    directory_handle_with_access(parent, FILE_READ_ATTRIBUTES | FILE_ADD_SUBDIRECTORY).is_some()
        && directory_handle_with_access(path, FILE_READ_ATTRIBUTES | DELETE_ACCESS).is_some()
}

fn directory_handle_with_access(path: &Path, access: u32) -> Option<std::fs::File> {
    let file = OpenOptions::new()
        .access_mode(access)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .ok()?;
    let information = file_handle_information(&file).ok()?;
    (information.attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
        && information.attributes & FILE_ATTRIBUTE_DIRECTORY != 0)
        .then_some(file)
}

fn run_preflight(paths: &InstallerPaths, context: &PreflightContext) -> AppResult<PreflightReport> {
    let architecture = std::env::consts::ARCH.to_owned();
    let supported_architecture = architecture == "x86_64";
    let os_build = windows_build_number();
    let supported_os = os_build.is_some_and(|build| build >= WINDOWS_11_MINIMUM_BUILD);
    let (standard_user_ok, standard_user_detail, standard_user_remediation) =
        standard_user_prerequisite(process_is_elevated());
    let build_environment = prepare_build_environment(context.minimum_cmake_version);
    let workspace = probe_workspace(paths, &context.target, context.operation_lock_held);
    let disk_budget = if workspace.is_ok() {
        inspect_disk_space_budget(paths, &context.target)
    } else {
        Err("Disk volumes cannot be measured until the workspace is writable.".into())
    };
    let free_bytes = disk_budget
        .as_ref()
        .map(DiskSpaceBudget::minimum_available)
        .unwrap_or(0);
    let disk_ok = disk_budget.as_ref().is_ok_and(DiskSpaceBudget::ready);

    let mut prerequisites = vec![
        prerequisite(
            "architecture",
            "Windows x64 architecture",
            supported_architecture,
            true,
            if supported_architecture {
                "Native x86_64 process and build target detected.".into()
            } else {
                format!("Detected {architecture}; only native Windows x64 is supported.")
            },
            (!supported_architecture).then(|| {
                "Use the x64 installer on a native x64 Windows 11 computer. Windows ARM emulation, WSL, and cross-compilation are not supported.".into()
            }),
        ),
        prerequisite(
            "osVersion",
            "Windows 11",
            supported_os,
            true,
            os_build
                .map(|build| format!("Windows build {build}."))
                .unwrap_or_else(|| "The native Windows build number could not be determined.".into()),
            (!supported_os).then(|| {
                "A supported Windows 11 x64 installation (build 22000 or newer) is required.".into()
            }),
        ),
        prerequisite(
            "nonElevated",
            "Standard user session",
            standard_user_ok,
            true,
            standard_user_detail,
            standard_user_remediation,
        ),
    ];

    match &build_environment {
        Ok(environment) => {
            prerequisites.push(prerequisite(
                "visualStudio",
                "Visual Studio 2022 C++ toolchain",
                true,
                true,
                format!("MSVC x64 compiler: {}", environment.cl.display()),
                None,
            ));
            prerequisites.push(prerequisite(
                "cmake",
                "CMake",
                true,
                true,
                environment.cmake.display().to_string(),
                None,
            ));
            prerequisites.push(prerequisite(
                "ninja",
                "Ninja",
                true,
                true,
                environment.ninja.display().to_string(),
                None,
            ));
        }
        Err(error) => {
            let detail = error
                .detail
                .clone()
                .unwrap_or_else(|| error.message.clone());
            prerequisites.push(prerequisite(
                "visualStudio",
                "Visual Studio 2022 C++ toolchain",
                false,
                true,
                detail,
                Some(format!(
                    "In Visual Studio Installer, install Desktop development with C++, the MSVC x64 tools, CMake tools, and Windows 11 SDK {WINDOWS_SDK_VERSION}. Then reopen the installer if PATH changed and check again."
                )),
            ));
        }
    }

    prerequisites.push(prerequisite(
        "workspace",
        "Writable local workspace and destination",
        workspace.is_ok(),
        true,
        workspace.clone().unwrap_or_else(|detail| detail),
        workspace.err().map(|_| {
            "Choose a local, user-writable destination and restore write/rename permissions for the installer data and cache folders. Network/UNC build folders are not supported.".into()
        }),
    ));
    prerequisites.push(prerequisite(
        "diskSpace",
        "Disk space on every installation volume",
        disk_ok,
        true,
        disk_budget
            .as_ref()
            .map(|budget| {
                format!(
                    "{}; {} bytes required on each distinct volume.",
                    budget.detail(),
                    BUILD_SAFETY_BUDGET_BYTES
                )
            })
            .unwrap_or_else(|detail| detail.clone()),
        (!disk_ok).then(|| {
            "Free at least 6 GiB on every distinct volume used by the cache/build workspace, destination, and backups, then check again.".into()
        }),
    ));

    let ready = prerequisites
        .iter()
        .all(|prerequisite| prerequisite.ok || !prerequisite.required);
    Ok(PreflightReport {
        ready,
        architecture,
        os_version: os_build
            .map(|build| format!("Windows build {build}"))
            .unwrap_or_else(|| "Windows (unknown build)".into()),
        free_bytes,
        minimum_free_bytes: BUILD_SAFETY_BUDGET_BYTES,
        homebrew_available: false,
        prerequisites,
    })
}

fn standard_user_prerequisite(elevation: Result<bool, String>) -> (bool, String, Option<String>) {
    match elevation {
        Ok(false) => (
            true,
            "The installer is running without administrator elevation.".into(),
            None,
        ),
        Ok(true) => (
            false,
            "This installer is running with an elevated administrator token.".into(),
            Some("Close this copy and reopen Aseprite Installer normally. It installs Aseprite only for the current user and never needs Run as administrator.".into()),
        ),
        Err(detail) => (
            false,
            format!("Administrator-token status could not be verified: {detail}"),
            Some("Close the installer, reopen it normally, and check requirements again. If the probe still fails, repair Windows PowerShell before installing.".into()),
        ),
    }
}

fn prerequisite(
    id: &str,
    label: &str,
    ok: bool,
    required: bool,
    detail: String,
    remediation: Option<String>,
) -> Prerequisite {
    Prerequisite {
        id: id.into(),
        label: label.into(),
        ok,
        required,
        detail,
        remediation,
    }
}

fn windows_build_number() -> Option<u64> {
    let powershell = trusted_system_binary(
        Path::new("System32/WindowsPowerShell/v1.0/powershell.exe"),
        "osVersion",
    )
    .ok()?;
    command_output_with_timeout(
        &powershell,
        [
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Environment]::OSVersion.Version.Build",
        ],
        Duration::from_secs(8),
    )
    .ok()?
    .lines()
    .find_map(|line| line.trim().parse().ok())
}

fn process_is_elevated() -> Result<bool, String> {
    let powershell = trusted_system_binary(
        Path::new("System32/WindowsPowerShell/v1.0/powershell.exe"),
        "elevation",
    )
    .map_err(|error| error.detail.unwrap_or(error.message))?;
    let output = command_output_with_timeout(
        &powershell,
        [
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)",
        ],
        Duration::from_secs(8),
    )?;
    Ok(output.trim().eq_ignore_ascii_case("true"))
}

fn probe_workspace(
    paths: &InstallerPaths,
    target: &Path,
    operation_lock_held: bool,
) -> Result<String, String> {
    if path_is_unc(target) {
        return Err(format!(
            "{} is a network/UNC destination; only local volumes are supported.",
            target.display()
        ));
    }
    for directory in [
        &paths.data_dir,
        &paths.cache_dir,
        &paths.archives_dir,
        &paths.builds_dir,
        &paths.logs_dir,
        &paths.backups_dir,
    ] {
        if path_is_unc(directory) {
            return Err(format!("{} is a network/UNC path.", directory.display()));
        }
        probe_directory_mutation(directory).map_err(|error| {
            format!(
                "{} cannot be changed atomically: {error}",
                directory.display()
            )
        })?;
    }
    if !operation_lock_held {
        let lock = paths.data_dir.join(".operation.lock");
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock)
            .map_err(|error| format!("{} cannot be locked: {error}", lock.display()))?;
        lock_file.try_lock_exclusive().map_err(|error| {
            format!(
                "{} is already locked by another installer operation: {error}",
                lock.display()
            )
        })?;
        FileExt::unlock(&lock_file)
            .map_err(|error| format!("{} could not be unlocked: {error}", lock.display()))?;
    }
    let destination = target
        .parent()
        .ok_or_else(|| "The target has no parent directory.".to_owned())?;
    probe_directory_mutation(destination).map_err(|error| {
        format!(
            "{} cannot be changed atomically: {error}",
            destination.display()
        )
    })?;
    Ok("Installer storage and the per-user destination support create/write/rename/delete operations.".into())
}

fn path_is_unc(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::Prefix(prefix)
                if matches!(prefix.kind(), Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _))
        )
    })
}

fn inspect_disk_space_budget(
    paths: &InstallerPaths,
    target: &Path,
) -> Result<DiskSpaceBudget, String> {
    let destination = target
        .parent()
        .ok_or_else(|| "The destination has no parent volume.".to_owned())?;
    let locations = [
        ("cache/build", paths.builds_dir.as_path()),
        ("destination", destination),
        ("backups", paths.backups_dir.as_path()),
    ];
    let mut grouped: BTreeMap<String, (PathBuf, Vec<&'static str>)> = BTreeMap::new();
    for (role, path) in locations {
        let measurable = path
            .ancestors()
            .find(|candidate| candidate.exists())
            .ok_or_else(|| format!("{} has no accessible volume.", path.display()))?;
        let canonical = std::fs::canonicalize(measurable).unwrap_or_else(|_| measurable.into());
        let volume = windows_volume_identity(&canonical).ok_or_else(|| {
            format!(
                "The volume containing {} could not be identified.",
                path.display()
            )
        })?;
        let entry = grouped
            .entry(volume)
            .or_insert_with(|| (canonical.clone(), Vec::new()));
        entry.1.push(role);
    }

    let mut volumes = Vec::with_capacity(grouped.len());
    for (volume, (path, roles)) in grouped {
        let available_bytes = available_space(&path).map_err(|error| {
            format!(
                "Free space on {volume} ({}) could not be measured: {error}",
                roles.join(", ")
            )
        })?;
        volumes.push(VolumeBudget {
            volume,
            roles,
            available_bytes,
        });
    }
    Ok(DiskSpaceBudget { volumes })
}

fn windows_volume_identity(path: &Path) -> Option<String> {
    let prefix = path.components().find_map(|component| match component {
        Component::Prefix(prefix) => Some(prefix.as_os_str()),
        _ => None,
    })?;
    Some(normalize_windows_identity_path(&prefix.to_string_lossy()))
}

pub fn probe_directory_mutation(directory: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let id = Uuid::new_v4();
    let source = directory.join(format!(".aseprite-installer-{id}.probe"));
    let destination = directory.join(format!(".aseprite-installer-{id}.renamed"));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&source)?;
        file.write_all(b"probe")?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&source, &destination)?;
        std::fs::remove_file(&destination)
    })();
    let _ = std::fs::remove_file(&source);
    let _ = std::fs::remove_file(&destination);
    result
}

pub fn prepare_build_environment(minimum_cmake_version: [u64; 3]) -> AppResult<BuildEnvironment> {
    let vswhere = locate_vswhere()?;
    let installation = command_output_with_timeout(
        &vswhere,
        [
            "-latest",
            "-products",
            "*",
            "-version",
            "[17.0,18.0)",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ],
        Duration::from_secs(15),
    )
    .map_err(|detail| {
        InstallerError::with_detail(
            "visualStudio",
            "Visual Studio Installer could not enumerate the C++ toolchain.",
            detail,
        )
    })?;
    let installation = installation
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .ok_or_else(|| {
            InstallerError::new(
                "visualStudio",
                "Visual Studio 2022 with the Desktop development with C++ workload was not found.",
            )
        })?;
    let installation = PathBuf::from(installation);
    let vcvars = installation
        .join("VC")
        .join("Auxiliary")
        .join("Build")
        .join("vcvarsall.bat");
    if !vcvars.is_file() {
        return Err(InstallerError::with_detail(
            "visualStudio",
            "Visual Studio's x64 developer environment script is missing.",
            vcvars.display().to_string(),
        ));
    }
    let command = format!(
        "\"\"{}\" amd64 {WINDOWS_SDK_VERSION} >nul && set\"",
        vcvars.display()
    );
    let cmd = trusted_system_binary(Path::new("System32/cmd.exe"), "visualStudio")?;
    let output = command_output_with_timeout_encoded(
        &cmd,
        ["/d", "/u", "/s", "/c", command.as_str()],
        Duration::from_secs(30),
        OutputEncoding::Utf16Le,
        VC_ENVIRONMENT_OUTPUT_LIMIT,
    )
    .map_err(|detail| {
        InstallerError::with_detail(
            "visualStudio",
            "The Visual Studio x64 developer environment could not be initialized.",
            detail,
        )
    })?;
    let mut environment = parse_vc_environment(&output);
    let system_root = trusted_windows_root("visualStudio")?;
    environment.insert(OsString::from("SystemRoot"), system_root.into_os_string());
    environment.insert(OsString::from("ComSpec"), cmd.into_os_string());
    for key in [
        "TEMP",
        "TMP",
        "LOCALAPPDATA",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "NUMBER_OF_PROCESSORS",
        "PATHEXT",
    ] {
        if let Some(value) = std::env::var_os(key) {
            environment.insert(OsString::from(key), value);
        }
    }
    let path = environment
        .iter()
        .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value.clone())
        .ok_or_else(|| {
            InstallerError::new("visualStudio", "Visual Studio did not provide PATH.")
        })?;
    let cl = find_executable_in_path("cl.exe", &path).ok_or_else(|| {
        InstallerError::new(
            "visualStudio",
            "The Visual Studio x64 environment does not contain cl.exe.",
        )
    })?;
    let cmake = find_tool_with_vs_fallback("cmake.exe", &path, &installation)?;
    let ninja = find_tool_with_vs_fallback("ninja.exe", &path, &installation)?;
    ensure_tool_version(&cmake, "cmake", minimum_cmake_version)?;
    ensure_tool_version(&ninja, "ninja", [1, 10, 0])?;
    let environment = BuildEnvironment {
        path,
        cl,
        cmake,
        ninja,
        environment,
    };
    functional_msvc_smoke_test(&environment)?;
    Ok(environment)
}

fn locate_vswhere() -> AppResult<PathBuf> {
    let root = std::env::var_os("ProgramFiles(x86)").ok_or_else(|| {
        InstallerError::new("visualStudio", "Windows did not provide ProgramFiles(x86).")
    })?;
    let path = PathBuf::from(root)
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe");
    path.is_file().then_some(path.clone()).ok_or_else(|| {
        InstallerError::with_detail(
            "visualStudio",
            "Visual Studio Installer was not found.",
            path.display().to_string(),
        )
    })
}

fn parse_vc_environment(output: &str) -> BTreeMap<OsString, OsString> {
    const ALLOWED: &[&str] = &[
        "PATH",
        "INCLUDE",
        "LIB",
        "LIBPATH",
        "WINDOWSSDKDIR",
        "WINDOWSSDKVERSION",
        "VCTOOLSINSTALLDIR",
        "UCRTVERSION",
        "UNIVERSALCRTSDKDIR",
        "VCINSTALLDIR",
        "VSINSTALLDIR",
        "VSCMD_ARG_APP_PLAT",
        "VSCMD_ARG_HOST_ARCH",
        "VSCMD_ARG_TGT_ARCH",
        "VSCMD_VER",
    ];
    output
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(key, _)| {
            ALLOWED
                .iter()
                .any(|allowed| key.eq_ignore_ascii_case(allowed))
        })
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect()
}

fn find_tool_with_vs_fallback(name: &str, path: &OsStr, installation: &Path) -> AppResult<PathBuf> {
    if let Some(executable) = find_executable_in_path(name, path) {
        return Ok(executable);
    }
    let candidates = if name.eq_ignore_ascii_case("cmake.exe") {
        vec![installation.join("Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin/cmake.exe")]
    } else {
        vec![installation.join("Common7/IDE/CommonExtensions/Microsoft/CMake/Ninja/ninja.exe")]
    };
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            InstallerError::new(
                "buildTool",
                format!("{name} was not found in PATH or the Visual Studio CMake tools component."),
            )
        })
}

fn find_executable_in_path(name: &str, path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn ensure_tool_version(path: &Path, name: &str, minimum: [u64; 3]) -> AppResult<()> {
    let output = command_output_with_timeout(path, ["--version"], Duration::from_secs(10))
        .map_err(|detail| {
            InstallerError::with_detail(
                "buildTool",
                format!("{name} could not be executed."),
                detail,
            )
        })?;
    let version = parse_numeric_version(&output).ok_or_else(|| {
        InstallerError::with_detail(
            "buildToolVersion",
            format!("{name} did not report a readable version."),
            output,
        )
    })?;
    if version < minimum {
        return Err(InstallerError::new(
            "buildToolVersion",
            format!(
                "{name} {}.{}.{} is too old; {}.{}.{} or newer is required.",
                version[0], version[1], version[2], minimum[0], minimum[1], minimum[2]
            ),
        ));
    }
    Ok(())
}

fn functional_msvc_smoke_test(environment: &BuildEnvironment) -> AppResult<()> {
    const SOURCE: &str = r#"#include <windows.h>
#if !defined(_M_X64)
#error The compiler target must be Windows x64.
#endif
static_assert(__cplusplus >= 201703L, "C++17 mode is required");
int main() {
  SYSTEM_INFO info{};
  GetNativeSystemInfo(&info);
  return info.wProcessorArchitecture == PROCESSOR_ARCHITECTURE_AMD64 ? 0 : 9;
}
"#;

    let temporary_root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("Temp"))
        .unwrap_or_else(std::env::temp_dir);
    if path_is_unc(&temporary_root) {
        return Err(InstallerError::with_detail(
            "toolchainSmoke",
            "The MSVC smoke test requires a local temporary directory.",
            temporary_root.display().to_string(),
        ));
    }
    std::fs::create_dir_all(&temporary_root)?;
    let workspace =
        temporary_root.join(format!(".aseprite-installer-msvc-smoke-{}", Uuid::new_v4()));
    std::fs::create_dir(&workspace)?;
    let source = workspace.join("smoke.cpp");
    let object = workspace.join("smoke.obj");
    let executable = workspace.join("smoke.exe");

    let result = (|| -> AppResult<()> {
        std::fs::write(&source, SOURCE.as_bytes())?;
        let mut executable_argument = OsString::from("/Fe");
        executable_argument.push(&executable);
        let mut object_argument = OsString::from("/Fo");
        object_argument.push(&object);
        let mut command = Command::new(&environment.cl);
        environment.configure_std(&mut command);
        command.current_dir(&workspace).args([
            OsString::from("/nologo"),
            OsString::from("/std:c++17"),
            OsString::from("/Zc:__cplusplus"),
            OsString::from("/permissive-"),
            OsString::from("/EHsc"),
            OsString::from("/W4"),
            OsString::from("/WX"),
            source.as_os_str().to_owned(),
            executable_argument,
            object_argument,
        ]);
        capture_command_output(
            command,
            &environment.cl,
            Duration::from_secs(45),
            OutputEncoding::Utf8Lossy,
            COMMAND_OUTPUT_LIMIT,
        )
        .map_err(|detail| {
            InstallerError::with_detail(
                "toolchainSmoke",
                "MSVC could not compile and link a C++17 x64 Windows SDK program.",
                detail,
            )
        })?;
        if !is_x64_pe(&executable)? {
            return Err(InstallerError::new(
                "toolchainSmoke",
                "MSVC's smoke-test output is not a Windows x64 PE executable.",
            ));
        }
        let mut command = Command::new(&executable);
        environment.configure_std(&mut command);
        command.current_dir(&workspace);
        capture_command_output(
            command,
            &executable,
            Duration::from_secs(15),
            OutputEncoding::Utf8Lossy,
            COMMAND_OUTPUT_LIMIT,
        )
        .map_err(|detail| {
            InstallerError::with_detail(
                "toolchainSmoke",
                "The compiled C++17 x64 Windows SDK smoke test did not run successfully.",
                detail,
            )
        })?;
        Ok(())
    })();
    let cleanup = std::fs::remove_dir_all(&workspace);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(InstallerError::with_detail(
            "toolchainSmokeCleanup",
            "The MSVC smoke-test workspace could not be removed.",
            format!("{}: {error}", workspace.display()),
        )),
    }
}

fn parse_numeric_version(value: &str) -> Option<[u64; 3]> {
    value
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .find_map(|candidate| {
            let mut parts = candidate.split('.').map(str::parse::<u64>);
            let major = parts.next()?.ok()?;
            let minor = parts.next()?.ok()?;
            let patch = parts.next().and_then(Result::ok).unwrap_or(0);
            Some([major, minor, patch])
        })
}

pub fn cmake_arguments(
    source: &Path,
    build: &Path,
    skia: &Path,
    environment: &BuildEnvironment,
) -> Vec<OsString> {
    vec![
        "-S".into(),
        source.as_os_str().into(),
        "-B".into(),
        build.as_os_str().into(),
        "-G".into(),
        "Ninja".into(),
        format!(
            "-DCMAKE_MAKE_PROGRAM:FILEPATH={}",
            environment.ninja.display()
        )
        .into(),
        "-DCMAKE_BUILD_TYPE:STRING=RelWithDebInfo".into(),
        format!("-DCMAKE_C_COMPILER:FILEPATH={}", environment.cl.display()).into(),
        format!("-DCMAKE_CXX_COMPILER:FILEPATH={}", environment.cl.display()).into(),
        "-DLAF_BACKEND:STRING=skia".into(),
        format!("-DSKIA_DIR:PATH={}", skia.display()).into(),
        format!(
            "-DSKIA_LIBRARY_DIR:PATH={}",
            skia.join("out/Release-x64").display()
        )
        .into(),
        format!(
            "-DSKIA_LIBRARY:FILEPATH={}",
            skia.join("out/Release-x64/skia.lib").display()
        )
        .into(),
        "-DENABLE_CCACHE:BOOL=OFF".into(),
        "-DENABLE_I18N_STRINGS:BOOL=OFF".into(),
        "-DENABLE_TESTS:BOOL=OFF".into(),
        "-DENABLE_BENCHMARKS:BOOL=OFF".into(),
    ]
}

pub fn find_built_artifact(build_dir: &Path) -> AppResult<PathBuf> {
    let bin = build_dir.join("bin");
    validate_artifact_structure(&bin)?;
    Ok(bin)
}

pub fn validate_artifact(root: &Path, expected_version: &str) -> AppResult<String> {
    let observed = artifact_version(root)?;
    let expected = parse_aseprite_version(expected_version).ok_or_else(|| {
        InstallerError::with_detail(
            "expectedVersion",
            "The expected Aseprite version is invalid.",
            expected_version,
        )
    })?;
    if observed != expected {
        return Err(InstallerError::with_detail(
            "artifactVersion",
            "The built executable did not report the selected Aseprite version.",
            format!("Expected {expected}; executable reported {observed}."),
        ));
    }
    Ok(observed)
}

/// Executes a structurally validated local Aseprite artifact only after the
/// caller has explicitly chosen to adopt or install it. Discovery must never
/// use this probe for untrusted candidates.
pub fn artifact_version(root: &Path) -> AppResult<String> {
    // Inspect and fingerprint the complete tree before executing any produced
    // code. This rejects junctions/reparse points and detects concurrent path
    // replacement before the version probe is allowed to run.
    artifact_fingerprint(root)?;
    validate_artifact_structure(root)?;
    let executable = root.join("aseprite.exe");
    if !is_x64_pe(&executable)? {
        return Err(InstallerError::new(
            "artifactArchitecture",
            "The built Aseprite executable is not a Windows x64 PE file.",
        ));
    }
    let output = command_output_with_timeout(&executable, ["--version"], Duration::from_secs(15))
        .map_err(|detail| {
        InstallerError::with_detail(
            "artifactLaunch",
            "The built Aseprite executable did not pass its launch validation.",
            detail,
        )
    })?;
    parse_aseprite_version(&output).ok_or_else(|| {
        InstallerError::with_detail(
            "artifactVersion",
            "The Aseprite executable returned an unrecognized version.",
            output,
        )
    })
}

fn validate_artifact_structure(root: &Path) -> AppResult<()> {
    for (path, directory) in [
        (root.join("aseprite.exe"), false),
        (root.join("data"), true),
        (root.join("icudtl.dat"), false),
    ] {
        let valid = std::fs::symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0)
            .is_some_and(|metadata| {
                if directory {
                    metadata.file_type().is_dir()
                } else {
                    metadata.file_type().is_file()
                }
            });
        if !valid {
            return Err(InstallerError::with_detail(
                "artifactIncomplete",
                "The built Aseprite artifact is incomplete.",
                format!("Missing or invalid {}", path.display()),
            ));
        }
    }
    Ok(())
}

fn open_no_follow(
    path: &Path,
    directory: bool,
    read_contents: bool,
    error_code: &str,
) -> AppResult<std::fs::File> {
    let mut options = OpenOptions::new();
    options
        .access_mode(if read_contents {
            GENERIC_READ
        } else {
            FILE_READ_ATTRIBUTES
        })
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        );
    options.open(path).map_err(|error| {
        InstallerError::with_detail(
            error_code,
            "A path could not be opened without following reparse points.",
            format!("{}: {error}", path.display()),
        )
    })
}

fn file_handle_information(file: &std::fs::File) -> std::io::Result<FileHandleInformation> {
    let mut raw = MaybeUninit::<ByHandleFileInformation>::uninit();
    // SAFETY: `file` owns a live Windows handle, and `raw` points to enough
    // writable memory for BY_HANDLE_FILE_INFORMATION. The API initializes the
    // complete structure when it reports success.
    let succeeded = unsafe { GetFileInformationByHandle(file.as_raw_handle(), raw.as_mut_ptr()) };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful call above initialized every field in `raw`.
    let raw = unsafe { raw.assume_init() };
    Ok(FileHandleInformation {
        identity: FileIdentity {
            volume_serial_number: raw.volume_serial_number,
            file_index: (u64::from(raw.file_index_high) << 32) | u64::from(raw.file_index_low),
        },
        attributes: raw.attributes,
        size: (u64::from(raw.file_size_high) << 32) | u64::from(raw.file_size_low),
        last_write_time: (u64::from(raw.last_write_time.high) << 32)
            | u64::from(raw.last_write_time.low),
    })
}

fn reject_reparse_point(path: &Path, metadata: &std::fs::Metadata) -> AppResult<()> {
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(InstallerError::with_detail(
            "artifactReparsePoint",
            "Managed artifacts cannot contain symbolic links, junctions, or other reparse points.",
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn reject_handle_reparse_point(path: &Path, information: FileHandleInformation) -> AppResult<()> {
    if information.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(InstallerError::with_detail(
            "artifactReparsePoint",
            "Managed artifacts cannot contain symbolic links, junctions, or other reparse points.",
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn same_file_snapshot(left: FileHandleInformation, right: FileHandleInformation) -> bool {
    left.identity == right.identity
        && left.attributes == right.attributes
        && left.size == right.size
        && left.last_write_time == right.last_write_time
}

fn is_x64_pe(path: &Path) -> AppResult<bool> {
    let mut file = open_no_follow(path, false, true, "pe")?;
    let before = file_handle_information(&file)?;
    if before.attributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0 {
        return Ok(false);
    }
    let mut dos = [0_u8; 64];
    file.read_exact(&mut dos)?;
    if &dos[..2] != b"MZ" {
        return Ok(false);
    }
    let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))?;
    let mut header = [0_u8; 6];
    file.read_exact(&mut header)?;
    let valid = &header[..4] == b"PE\0\0" && u16::from_le_bytes([header[4], header[5]]) == 0x8664;
    let after = file_handle_information(&file)?;
    let current = open_no_follow(path, false, false, "pe")?;
    let current = file_handle_information(&current)?;
    if !same_file_snapshot(before, after) || !same_file_snapshot(after, current) {
        return Err(InstallerError::with_detail(
            "artifactChanged",
            "The executable changed while it was being inspected.",
            path.display().to_string(),
        ));
    }
    Ok(valid)
}

pub fn artifact_fingerprint(root: &Path) -> AppResult<[u8; 32]> {
    let root_metadata = std::fs::symlink_metadata(root).map_err(|error| {
        InstallerError::with_detail(
            "fingerprint",
            "The installation root could not be inspected.",
            format!("{}: {error}", root.display()),
        )
    })?;
    reject_reparse_point(root, &root_metadata)?;
    if !root_metadata.file_type().is_dir() && !root_metadata.file_type().is_file() {
        return Err(InstallerError::with_detail(
            "artifactType",
            "The installation root must be a regular file or directory.",
            root.display().to_string(),
        ));
    }
    let root_handle = open_no_follow(
        root,
        root_metadata.file_type().is_dir(),
        false,
        "fingerprint",
    )?;
    let root_information = file_handle_information(&root_handle)?;
    reject_handle_reparse_point(root, root_information)?;

    // Inspect a directory before asking WalkDir to descend into it. This is
    // important on Windows because junctions are reparse points and must never
    // become traversal roots, even when a library does not classify one as a
    // conventional symbolic link.
    let mut walker = WalkDir::new(root).follow_links(false).into_iter();
    let mut entries = Vec::new();
    while let Some(entry) = walker.next() {
        let entry = entry.map_err(|error| {
            InstallerError::with_detail(
                "fingerprint",
                "The complete installation tree could not be inspected.",
                error.to_string(),
            )
        })?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            walker.skip_current_dir();
            return Err(InstallerError::with_detail(
                "artifactReparsePoint",
                "Managed artifacts cannot contain symbolic links, junctions, or other reparse points.",
                entry.path().display().to_string(),
            ));
        }
        entries.push(entry);
    }
    entries.sort_by_key(|entry| entry.path().to_string_lossy().to_ascii_lowercase());
    let mut seen = BTreeMap::new();
    let mut hasher = Sha256::new();
    hasher.update(b"aseprite-installer/windows-artifact-fingerprint/v2\0");
    let mut directory_identities = Vec::new();
    for entry in entries {
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(path)?;
        reject_reparse_point(path, &metadata)?;
        let file_type = metadata.file_type();
        if !file_type.is_dir() && !file_type.is_file() {
            return Err(InstallerError::new(
                "artifactSpecialFile",
                "Managed artifacts cannot contain special files.",
            ));
        }
        let normalized = normalized_artifact_relative(root, path)?;
        let kind = if file_type.is_dir() { b'd' } else { b'f' };
        if seen.insert(normalized.clone(), kind).is_some() {
            return Err(InstallerError::new(
                "artifactCollision",
                "The artifact contains case-colliding paths that are unsafe on Windows.",
            ));
        }
        let path_bytes = normalized.as_bytes();
        let path_length = u64::try_from(path_bytes.len()).map_err(|_| {
            InstallerError::new(
                "fingerprint",
                "An artifact path is too long to fingerprint.",
            )
        })?;
        hasher.update([kind]);
        hasher.update(path_length.to_le_bytes());
        hasher.update(path_bytes);

        if file_type.is_file() {
            let mut file = open_no_follow(path, false, true, "fingerprint")?;
            let before = file_handle_information(&file)?;
            reject_handle_reparse_point(path, before)?;
            if path == root && !same_file_snapshot(before, root_information) {
                return Err(InstallerError::with_detail(
                    "artifactChanged",
                    "The artifact root changed before it could be fingerprinted.",
                    path.display().to_string(),
                ));
            }
            if before.attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                return Err(InstallerError::with_detail(
                    "artifactChanged",
                    "An artifact file became a directory during inspection.",
                    path.display().to_string(),
                ));
            }
            hasher.update(before.size.to_le_bytes());
            let mut buffer = [0_u8; 128 * 1024];
            let mut bytes_read = 0_u64;
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                bytes_read = bytes_read.checked_add(count as u64).ok_or_else(|| {
                    InstallerError::new("fingerprint", "Artifact size overflowed.")
                })?;
                hasher.update(&buffer[..count]);
            }
            let after = file_handle_information(&file)?;
            let current = open_no_follow(path, false, false, "fingerprint")?;
            let current = file_handle_information(&current)?;
            if bytes_read != before.size
                || !same_file_snapshot(before, after)
                || !same_file_snapshot(after, current)
            {
                return Err(InstallerError::with_detail(
                    "artifactChanged",
                    "The artifact changed while it was being fingerprinted.",
                    path.display().to_string(),
                ));
            }
        } else {
            let directory = open_no_follow(path, true, false, "fingerprint")?;
            let information = file_handle_information(&directory)?;
            reject_handle_reparse_point(path, information)?;
            if path == root && !same_file_snapshot(information, root_information) {
                return Err(InstallerError::with_detail(
                    "artifactChanged",
                    "The artifact root changed before it could be fingerprinted.",
                    path.display().to_string(),
                ));
            }
            if information.attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                return Err(InstallerError::with_detail(
                    "artifactChanged",
                    "An artifact directory became a file during inspection.",
                    path.display().to_string(),
                ));
            }
            hasher.update(0_u64.to_le_bytes());
            directory_identities.push((path.to_path_buf(), information));
        }
    }

    for (path, expected) in directory_identities {
        let directory = open_no_follow(&path, true, false, "fingerprint")?;
        let current = file_handle_information(&directory)?;
        reject_handle_reparse_point(&path, current)?;
        if !same_file_snapshot(current, expected) {
            return Err(InstallerError::with_detail(
                "artifactChanged",
                "An artifact directory changed while it was being fingerprinted.",
                path.display().to_string(),
            ));
        }
    }
    if collect_artifact_manifest(root)? != seen {
        return Err(InstallerError::with_detail(
            "artifactChanged",
            "The artifact gained, lost, or changed an entry while it was being fingerprinted.",
            root.display().to_string(),
        ));
    }
    Ok(hasher.finalize().into())
}

fn normalized_artifact_relative(root: &Path, path: &Path) -> AppResult<String> {
    let relative = path.strip_prefix(root).map_err(|error| {
        InstallerError::with_detail(
            "fingerprint",
            "The installation path is inconsistent.",
            error.to_string(),
        )
    })?;
    Ok(relative
        .to_str()
        .ok_or_else(|| {
            InstallerError::with_detail(
                "artifactPathEncoding",
                "Managed artifact paths must contain valid Unicode.",
                path.display().to_string(),
            )
        })?
        .replace('\\', "/")
        .to_ascii_lowercase())
}

fn collect_artifact_manifest(root: &Path) -> AppResult<BTreeMap<String, u8>> {
    let mut manifest = BTreeMap::new();
    let mut walker = WalkDir::new(root).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        let entry = entry.map_err(|error| {
            InstallerError::with_detail(
                "fingerprint",
                "The complete installation tree could not be re-inspected.",
                error.to_string(),
            )
        })?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            walker.skip_current_dir();
            return Err(InstallerError::with_detail(
                "artifactReparsePoint",
                "Managed artifacts cannot contain symbolic links, junctions, or other reparse points.",
                path.display().to_string(),
            ));
        }
        let kind = if metadata.file_type().is_dir() {
            b'd'
        } else if metadata.file_type().is_file() {
            b'f'
        } else {
            return Err(InstallerError::with_detail(
                "artifactSpecialFile",
                "Managed artifacts cannot contain special files.",
                path.display().to_string(),
            ));
        };
        let normalized = normalized_artifact_relative(root, path)?;
        if manifest.insert(normalized, kind).is_some() {
            return Err(InstallerError::new(
                "artifactCollision",
                "The artifact contains case-colliding paths that are unsafe on Windows.",
            ));
        }
    }
    Ok(manifest)
}

pub fn installation_id(path: &str) -> String {
    let normalized = normalize_windows_identity_path(path);
    format!(
        "windows-{}",
        hex::encode(Sha256::digest(normalized.as_bytes()))
    )
}

fn normalize_windows_identity_path(path: &str) -> String {
    let mut normalized = path.replace('/', "\\");
    if normalized
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("\\\\?\\UNC\\"))
    {
        normalized = format!("\\\\{}", &normalized[8..]);
    } else if normalized
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("\\\\?\\"))
    {
        normalized.drain(..4);
    }
    normalized.trim_end_matches('\\').to_ascii_lowercase()
}

pub fn target_aseprite_running(path: &Path) -> AppResult<bool> {
    let candidate = if path.is_dir() {
        path.join("aseprite.exe")
    } else {
        path.to_path_buf()
    };
    let expected = match std::fs::canonicalize(&candidate) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(InstallerError::with_detail(
                "processProbe",
                "The target Aseprite executable could not be identified before checking running processes.",
                format!("{}: {error}", candidate.display()),
            ))
        }
    };
    let powershell = trusted_system_binary(
        Path::new("System32/WindowsPowerShell/v1.0/powershell.exe"),
        "processProbe",
    )?;
    let script = "$ErrorActionPreference='Stop'; Get-Process -Name aseprite -ErrorAction SilentlyContinue | ForEach-Object { $p=$_.Path; if ([string]::IsNullOrWhiteSpace($p)) { throw 'Aseprite process path unavailable' }; [Console]::Out.WriteLine(([BitConverter]::ToString([Text.Encoding]::Unicode.GetBytes($p))).Replace('-','')) }";
    let output = command_output_with_timeout(
        &powershell,
        ["-NoProfile", "-NonInteractive", "-Command", script],
        Duration::from_secs(12),
    )
    .map_err(|detail| {
        InstallerError::with_detail(
            "processProbe",
            "Windows could not safely identify running Aseprite process paths.",
            detail,
        )
    })?;
    for encoded in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let bytes = hex::decode(encoded).map_err(|error| {
            InstallerError::with_detail(
                "processProbe",
                "Windows returned an invalid encoded process path.",
                error.to_string(),
            )
        })?;
        let running = decode_utf16_le(&bytes).map_err(|detail| {
            InstallerError::with_detail(
                "processProbe",
                "Windows returned an invalid process path.",
                detail,
            )
        })?;
        let running = std::fs::canonicalize(running.trim()).map_err(|error| {
            InstallerError::with_detail(
                "processProbe",
                "A running Aseprite executable path could not be verified.",
                error.to_string(),
            )
        })?;
        if paths_equal(&running, &expected) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn launch(path: &Path) -> AppResult<()> {
    let candidate = if path.is_dir() {
        path.join("aseprite.exe")
    } else {
        path.to_path_buf()
    };
    let executable = std::fs::canonicalize(&candidate).map_err(|error| {
        InstallerError::with_detail(
            "launch",
            "The selected Aseprite executable is unavailable.",
            format!("{}: {error}", candidate.display()),
        )
    })?;
    if !executable
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("aseprite.exe"))
        || !is_x64_pe(&executable)?
    {
        return Err(InstallerError::with_detail(
            "launch",
            "The selected file is not a valid Windows x64 Aseprite executable.",
            executable.display().to_string(),
        ));
    }
    let mut command = tokio::process::Command::new(&executable);
    sanitize_runtime_command(&mut command);
    command
        .current_dir(executable.parent().ok_or_else(|| {
            InstallerError::new("launch", "The Aseprite executable has no parent directory.")
        })?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    let mut child = command.spawn().map_err(|error| {
        InstallerError::with_detail(
            "launch",
            "Aseprite could not be launched.",
            format!("{}: {error}", executable.display()),
        )
    })?;
    tauri::async_runtime::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}

pub async fn reveal(path: &Path) -> AppResult<()> {
    let visible = std::fs::canonicalize(path).map_err(|error| {
        InstallerError::with_detail(
            "reveal",
            "The selected installation location is unavailable.",
            format!("{}: {error}", path.display()),
        )
    })?;
    let explorer = trusted_explorer()?;
    let mut command = tokio::process::Command::new(&explorer);
    sanitize_runtime_command(&mut command);
    command
        .arg("/select,")
        .arg(&visible)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = tokio::time::timeout(Duration::from_secs(15), command.status())
        .await
        .map_err(|_| {
            InstallerError::new(
                "revealTimeout",
                "File Explorer did not accept the reveal request in time.",
            )
        })?
        .map_err(|error| {
            InstallerError::with_detail(
                "reveal",
                "The installation location could not be shown in File Explorer.",
                error.to_string(),
            )
        })?;
    if !status.success() {
        return Err(InstallerError::with_detail(
            "reveal",
            "File Explorer rejected the reveal request.",
            format!("{} exited with {status}", explorer.display()),
        ));
    }
    Ok(())
}

pub async fn open_external(url: &str) -> AppResult<()> {
    let parsed = url::Url::parse(url).map_err(|_| {
        InstallerError::new("externalUrl", "The requested external URL is invalid.")
    })?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err(InstallerError::new(
            "externalUrl",
            "Only absolute HTTPS documentation URLs can be opened.",
        ));
    }
    let explorer = trusted_explorer()?;
    let mut command = tokio::process::Command::new(&explorer);
    sanitize_runtime_command(&mut command);
    command
        .arg(parsed.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = tokio::time::timeout(Duration::from_secs(15), command.status())
        .await
        .map_err(|_| {
            InstallerError::new(
                "openTimeout",
                "File Explorer did not accept the external URL in time.",
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
            "File Explorer rejected the external URL.",
            format!("{} exited with {status}", explorer.display()),
        ));
    }
    Ok(())
}

fn trusted_explorer() -> AppResult<PathBuf> {
    trusted_system_binary(Path::new("explorer.exe"), "explorer")
}

fn trusted_system_binary(relative: &Path, error_code: &str) -> AppResult<PathBuf> {
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(InstallerError::new(
            error_code,
            "A trusted Windows system executable path is invalid.",
        ));
    }
    let system_root = trusted_windows_root(error_code)?;
    let executable = std::fs::canonicalize(system_root.join(relative)).map_err(|error| {
        InstallerError::with_detail(
            error_code,
            "A trusted Windows system executable is unavailable.",
            error.to_string(),
        )
    })?;
    if !executable.starts_with(&system_root) || !is_x64_pe(&executable)? {
        return Err(InstallerError::with_detail(
            error_code,
            "A trusted Windows system executable could not be verified.",
            executable.display().to_string(),
        ));
    }
    Ok(executable)
}

fn trusted_windows_root(error_code: &str) -> AppResult<PathBuf> {
    let mut buffer = vec![0_u16; 32_768];
    let length = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return Err(InstallerError::with_detail(
            error_code,
            "Windows did not provide its trusted system directory.",
            std::io::Error::last_os_error().to_string(),
        ));
    }
    buffer.truncate(length as usize);
    let system_root = PathBuf::from(OsString::from_wide(&buffer));
    let system_root = std::fs::canonicalize(&system_root).map_err(|error| {
        InstallerError::with_detail(
            error_code,
            "The Windows SystemRoot directory could not be verified.",
            format!("{}: {error}", system_root.display()),
        )
    })?;
    if !system_root.is_absolute() {
        return Err(InstallerError::with_detail(
            error_code,
            "Windows provided a non-absolute system directory.",
            system_root.display().to_string(),
        ));
    }
    Ok(system_root)
}

fn sanitize_runtime_command(command: &mut tokio::process::Command) {
    for variable in [
        "__COMPAT_LAYER",
        "ASEPRITE_USER_FOLDER",
        "QT_PLUGIN_PATH",
        "QT_QPA_PLATFORM_PLUGIN_PATH",
        "SSLKEYLOGFILE",
    ] {
        command.env_remove(variable);
    }
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
    let layout = validate_windows_integration_paths(paths, installation_id)?;
    let shortcut = layout.shortcut_path;
    let snapshot = snapshot_shortcut(&shortcut)?;
    if snapshot.contents.is_some() && !shortcut_is_owned(&shortcut, installation_id)? {
        return Err(InstallerError::with_detail(
            "desktopIntegrationOwnership",
            "The per-user Start menu shortcut path is occupied by a file not owned by this installer.",
            shortcut.display().to_string(),
        ));
    }
    Ok(vec![snapshot])
}

pub fn prepare_desktop_integration(
    root: &Path,
    installation_id: &str,
    paths: &[PathBuf],
) -> AppResult<Vec<IntegrationSnapshot>> {
    validate_integration_id(installation_id)?;
    let layout = validate_windows_integration_paths(paths, installation_id)?;
    let shortcut = layout.shortcut_path;
    let programs = shortcut.parent().ok_or_else(|| {
        InstallerError::new(
            "desktopIntegration",
            "The Start menu shortcut path has no parent directory.",
        )
    })?;
    ensure_windows_integration_directory(&layout.confinement_root, programs)?;
    let root = std::fs::canonicalize(root)?;
    let executable = root.join("aseprite.exe");
    if !is_x64_pe(&executable)? {
        return Err(InstallerError::new(
            "desktopIntegration",
            "The Start menu shortcut target is not a Windows x64 Aseprite executable.",
        ));
    }
    let powershell = trusted_system_binary(
        Path::new("System32/WindowsPowerShell/v1.0/powershell.exe"),
        "desktopIntegration",
    )?;
    let script = format!(
        "$ErrorActionPreference='Stop'; $w=New-Object -ComObject WScript.Shell; $s=$w.CreateShortcut($args[0]); $s.TargetPath=$args[1]; $s.WorkingDirectory=$args[2]; $s.Description=('{SHORTCUT_DESCRIPTION_PREFIX}'+$args[3]+')'); $s.Save()"
    );
    let temporary = programs.join(format!(
        ".aseprite-installer-shortcut-{}.lnk",
        Uuid::new_v4()
    ));
    let create_result = command_output_with_timeout(
        &powershell,
        [
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-Command"),
            OsString::from(script),
            temporary.as_os_str().to_owned(),
            executable.as_os_str().to_owned(),
            root.as_os_str().to_owned(),
            OsString::from(installation_id),
        ],
        Duration::from_secs(15),
    )
    .map_err(|detail| {
        InstallerError::with_detail(
            "desktopIntegration",
            "The per-user Start menu shortcut could not be created.",
            detail,
        )
    });
    let result = (|| {
        create_result?;
        validate_windows_path_chain(&layout.confinement_root, programs)?;
        let metadata = std::fs::symlink_metadata(&temporary)?;
        if !metadata.file_type().is_file()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !shortcut_is_owned(&temporary, installation_id)?
        {
            return Err(InstallerError::with_detail(
                "desktopIntegrationOwnership",
                "The staged Start menu shortcut could not be verified as installer-owned.",
                temporary.display().to_string(),
            ));
        }
        let snapshot = snapshot_shortcut(&temporary)?;
        Ok(vec![IntegrationSnapshot::file(
            shortcut.clone(),
            snapshot.contents.unwrap_or_default(),
            None,
        )?])
    })();
    let _ = std::fs::remove_file(&temporary);
    result
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
    if desired_paths != alternative_paths {
        return Err(InstallerError::new(
            "desktopIntegrationSnapshot",
            "Start menu snapshots do not describe the same managed path.",
        ));
    }
    if desired.is_empty() {
        return Ok(Vec::new());
    }
    let layout = validate_windows_integration_paths(&desired_paths, installation_id)?;
    let programs = layout.shortcut_path.parent().unwrap();
    ensure_windows_integration_directory(&layout.confinement_root, programs)?;
    let desired = &desired[0];
    let alternative = &alternative[0];
    desired.validate()?;
    alternative.validate()?;
    let current = snapshot_shortcut(&desired.path)?;
    if current.contents == desired.contents && current.sha256 == desired.sha256 {
        return Ok(desired
            .contents
            .as_ref()
            .map(|_| vec![desired.path.clone()])
            .unwrap_or_default());
    }
    if current.contents != alternative.contents || current.sha256 != alternative.sha256 {
        return Err(InstallerError::with_detail(
            "desktopIntegrationConflict",
            "The Start menu shortcut changed outside this transaction and was preserved.",
            desired.path.display().to_string(),
        ));
    }
    match desired.contents.as_deref() {
        Some(contents) => atomic_write_shortcut(&desired.path, contents)?,
        None => match std::fs::remove_file(&desired.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        },
    }
    let activated = snapshot_shortcut(&desired.path)?;
    if activated.contents != desired.contents || activated.sha256 != desired.sha256 {
        return Err(InstallerError::with_detail(
            "desktopIntegrationConflict",
            "The activated Start menu snapshot changed unexpectedly.",
            desired.path.display().to_string(),
        ));
    }
    Ok(desired
        .contents
        .as_ref()
        .map(|_| vec![desired.path.clone()])
        .unwrap_or_default())
}

fn snapshot_shortcut(path: &Path) -> AppResult<IntegrationSnapshot> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(IntegrationSnapshot::absent(path.to_path_buf()))
        }
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(InstallerError::with_detail(
            "desktopIntegrationType",
            "A Start menu snapshot path is not a regular non-reparse file.",
            path.display().to_string(),
        ));
    }
    if metadata.len() > MAX_INTEGRATION_FILE_BYTES as u64 {
        return Err(InstallerError::with_detail(
            "desktopIntegrationSize",
            "A Start menu shortcut exceeds the transaction snapshot safety limit.",
            path.display().to_string(),
        ));
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path)?;
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_INTEGRATION_FILE_BYTES + 1) as u64)
        .read_to_end(&mut contents)?;
    if contents.len() > MAX_INTEGRATION_FILE_BYTES {
        return Err(InstallerError::new(
            "desktopIntegrationSize",
            "A Start menu shortcut grew beyond its snapshot safety limit.",
        ));
    }
    IntegrationSnapshot::file(path.to_path_buf(), contents, None)
}

fn atomic_write_shortcut(path: &Path, contents: &[u8]) -> AppResult<()> {
    let parent = path.parent().ok_or_else(|| {
        InstallerError::new(
            "desktopIntegration",
            "The Start menu shortcut has no parent directory.",
        )
    })?;
    let temporary = parent.join(format!(
        ".aseprite-installer-shortcut-{}.tmp",
        Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options
            .create_new(true)
            .write(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        let mut file = options.open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        if path.exists() {
            match crate::state::replace_file_durable(&temporary, path)? {
                crate::state::CommitDurability::Durable => {}
                crate::state::CommitDurability::Uncertain(detail) => {
                    return Err(InstallerError::with_detail(
                        "desktopIntegrationDurability",
                        "The Start menu shortcut was replaced but could not be proven durable.",
                        detail,
                    ))
                }
            }
        } else {
            durable_rename_file_no_replace(&temporary, path)?;
        }
        Ok::<(), InstallerError>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub fn desktop_integration_paths(installation_id: &str) -> AppResult<Vec<PathBuf>> {
    validate_integration_id(installation_id)?;
    let app_data = known_folder_path(&FOLDER_ID_ROAMING_APP_DATA, "desktopIntegration")?;
    Ok(vec![app_data
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join(format!(
            "{SHORTCUT_PREFIX}{installation_id}{SHORTCUT_SUFFIX}"
        ))])
}

pub fn validate_desktop_integration_paths(
    paths: &[PathBuf],
    installation_id: &str,
) -> AppResult<()> {
    if paths.is_empty() {
        validate_integration_id(installation_id)?;
        return Ok(());
    }
    validate_windows_integration_paths(paths, installation_id).map(|_| ())
}

#[derive(Debug)]
struct WindowsIntegrationLayout {
    confinement_root: PathBuf,
    shortcut_path: PathBuf,
}

fn validate_windows_integration_paths(
    paths: &[PathBuf],
    installation_id: &str,
) -> AppResult<WindowsIntegrationLayout> {
    validate_integration_id(installation_id)?;
    if paths.len() != 1 {
        return Err(InstallerError::with_detail(
            "desktopIntegrationPath",
            "The Start menu integration path set is incomplete or contains unexpected entries.",
            format!("Expected one managed path; received {}.", paths.len()),
        ));
    }
    let path = &paths[0];
    validate_windows_absolute_normal_path(path)?;
    let programs = path.parent().ok_or_else(|| {
        InstallerError::with_detail(
            "desktopIntegrationPath",
            "A stored Start menu path has no parent directory.",
            path.display().to_string(),
        )
    })?;
    let app_data_root = programs.ancestors().nth(4).ok_or_else(|| {
        InstallerError::with_detail(
            "desktopIntegrationPath",
            "A stored Start menu path has no per-user AppData root.",
            path.display().to_string(),
        )
    })?;
    let expected = app_data_root
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join(format!(
            "{SHORTCUT_PREFIX}{installation_id}{SHORTCUT_SUFFIX}"
        ));
    if !windows_paths_lexically_equal(path, &expected) {
        return Err(InstallerError::with_detail(
            "desktopIntegrationPath",
            "A stored Start menu path does not have the deterministic managed name and location.",
            path.display().to_string(),
        ));
    }

    let profile = known_folder_path(&FOLDER_ID_PROFILE, "desktopIntegrationPath")?;
    let roaming = known_folder_path(&FOLDER_ID_ROAMING_APP_DATA, "desktopIntegrationPath")?;
    validate_windows_absolute_normal_path(&profile)?;
    validate_windows_absolute_normal_path(&roaming)?;
    let confinement_root = if windows_paths_lexically_equal(app_data_root, &roaming) {
        roaming
    } else if windows_path_is_within(app_data_root, &profile) {
        profile
    } else {
        return Err(InstallerError::with_detail(
            "desktopIntegrationPath",
            "A stored Start menu path is outside the current user's known profile folders.",
            path.display().to_string(),
        ));
    };
    validate_windows_path_chain(&confinement_root, programs)?;
    Ok(WindowsIntegrationLayout {
        confinement_root,
        shortcut_path: expected,
    })
}

fn known_folder_path(folder_id: &Guid, error_code: &str) -> AppResult<PathBuf> {
    let mut raw = std::ptr::null_mut::<u16>();
    let result = unsafe {
        SHGetKnownFolderPath(
            folder_id,
            0,
            std::ptr::null_mut(),
            &mut raw as *mut *mut u16,
        )
    };
    if result < 0 || raw.is_null() {
        if !raw.is_null() {
            unsafe { CoTaskMemFree(raw.cast()) };
        }
        return Err(InstallerError::with_detail(
            error_code,
            "Windows could not resolve a required per-user known folder.",
            format!(
                "SHGetKnownFolderPath returned HRESULT 0x{:08x}.",
                result as u32
            ),
        ));
    }

    let mut length = 0_usize;
    while length < 32_768 && unsafe { *raw.add(length) } != 0 {
        length += 1;
    }
    let path = if length == 32_768 {
        None
    } else {
        let wide = unsafe { std::slice::from_raw_parts(raw, length) };
        Some(PathBuf::from(OsString::from_wide(wide)))
    };
    unsafe { CoTaskMemFree(raw.cast()) };
    let path = path.ok_or_else(|| {
        InstallerError::new(
            error_code,
            "Windows returned an unterminated per-user known-folder path.",
        )
    })?;
    if !path.is_absolute() {
        return Err(InstallerError::with_detail(
            error_code,
            "Windows returned a non-absolute per-user known-folder path.",
            path.display().to_string(),
        ));
    }
    Ok(path)
}

fn validate_windows_absolute_normal_path(path: &Path) -> AppResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(InstallerError::with_detail(
            "desktopIntegrationPath",
            "A Start menu path must be absolute and contain no traversal components.",
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn windows_paths_lexically_equal(left: &Path, right: &Path) -> bool {
    normalize_windows_identity_path(&left.to_string_lossy())
        == normalize_windows_identity_path(&right.to_string_lossy())
}

fn windows_path_is_within(candidate: &Path, root: &Path) -> bool {
    let candidate = normalize_windows_identity_path(&candidate.to_string_lossy());
    let root = normalize_windows_identity_path(&root.to_string_lossy());
    candidate == root
        || candidate
            .strip_prefix(&root)
            .is_some_and(|suffix| suffix.starts_with('\\'))
}

fn validate_windows_path_chain(confinement_root: &Path, stop: &Path) -> AppResult<()> {
    validate_windows_absolute_normal_path(stop)?;
    if !windows_path_is_within(stop, confinement_root) {
        return Err(InstallerError::with_detail(
            "desktopIntegrationPath",
            "A Start menu path escaped its per-user known-folder confinement.",
            stop.display().to_string(),
        ));
    }
    let mut current = PathBuf::new();
    for component in stop.components() {
        current.push(component.as_os_str());
        if !current.is_absolute() {
            continue;
        }
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(InstallerError::with_detail(
                "desktopIntegrationDirectory",
                "A Start menu directory chain contains a reparse point or non-directory.",
                current.display().to_string(),
            ));
        }
    }
    Ok(())
}

fn ensure_windows_integration_directory(
    confinement_root: &Path,
    directory: &Path,
) -> AppResult<()> {
    validate_windows_path_chain(confinement_root, directory)?;
    let mut current = PathBuf::new();
    for component in directory.components() {
        current.push(component.as_os_str());
        if !current.is_absolute() {
            continue;
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata)
                if metadata.is_dir()
                    && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 => {}
            Ok(_) => {
                return Err(InstallerError::with_detail(
                    "desktopIntegrationDirectory",
                    "A Start menu directory is a reparse point or non-directory.",
                    current.display().to_string(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !windows_path_is_within(&current, confinement_root) {
                    return Err(InstallerError::with_detail(
                        "desktopIntegrationDirectory",
                        "A required known-folder ancestor does not exist.",
                        current.display().to_string(),
                    ));
                }
                std::fs::create_dir(&current)?;
                let metadata = std::fs::symlink_metadata(&current)?;
                if !metadata.is_dir()
                    || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
                {
                    return Err(InstallerError::with_detail(
                        "desktopIntegrationDirectory",
                        "A newly created Start menu directory could not be secured.",
                        current.display().to_string(),
                    ));
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    validate_windows_path_chain(confinement_root, directory)
}

fn validate_integration_id(installation_id: &str) -> AppResult<()> {
    if installation_id.is_empty()
        || installation_id.len() > 96
        || !installation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(InstallerError::new(
            "desktopIntegrationId",
            "The installation identity is not safe for a Start menu shortcut.",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn shortcut_installation_id(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    let installation_id = name
        .strip_prefix(SHORTCUT_PREFIX)?
        .strip_suffix(SHORTCUT_SUFFIX)?;
    validate_integration_id(installation_id)
        .is_ok()
        .then_some(installation_id)
}

fn shortcut_is_owned(path: &Path, installation_id: &str) -> AppResult<bool> {
    validate_integration_id(installation_id)?;
    let powershell = trusted_system_binary(
        Path::new("System32/WindowsPowerShell/v1.0/powershell.exe"),
        "desktopIntegrationOwnership",
    )?;
    let script = format!(
        "$ErrorActionPreference='Stop'; $w=New-Object -ComObject WScript.Shell; $s=$w.CreateShortcut($args[0]); $expected=('{SHORTCUT_DESCRIPTION_PREFIX}'+$args[1]+')'); if ($s.Description -ceq $expected) {{ [Console]::Out.Write('owned') }} else {{ [Console]::Out.Write('foreign') }}"
    );
    let output = command_output_with_timeout(
        &powershell,
        [
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-Command"),
            OsString::from(script),
            path.as_os_str().to_owned(),
            OsString::from(installation_id),
        ],
        Duration::from_secs(15),
    )
    .map_err(|detail| {
        InstallerError::with_detail(
            "desktopIntegrationOwnership",
            "Start menu shortcut ownership could not be inspected.",
            detail,
        )
    })?;
    Ok(output.trim() == "owned")
}

fn command_output_with_timeout<I, S>(
    program: &Path,
    arguments: I,
    timeout: Duration,
) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    command_output_with_timeout_encoded(
        program,
        arguments,
        timeout,
        OutputEncoding::Utf8Lossy,
        COMMAND_OUTPUT_LIMIT,
    )
}

#[derive(Debug, Clone, Copy)]
enum OutputEncoding {
    Utf8Lossy,
    Utf16Le,
}

#[derive(Debug)]
struct BoundedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

fn command_output_with_timeout_encoded<I, S>(
    program: &Path,
    arguments: I,
    timeout: Duration,
    encoding: OutputEncoding,
    output_limit: usize,
) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command.args(arguments);
    capture_command_output(command, program, timeout, encoding, output_limit)
}

fn capture_command_output(
    mut command: Command,
    program: &Path,
    timeout: Duration,
    encoding: OutputEncoding,
    output_limit: usize,
) -> Result<String, String> {
    let job = ProcessTreeJob::new().map_err(|error| error.to_string())?;
    ProcessTreeJob::prepare_std_command(&mut command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{} could not start: {error}", program.display()))?;
    if let Err(error) = job.assign_and_resume_std_child(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.to_string());
    }
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = job.terminate();
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{} did not provide stdout.", program.display()));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = job.terminate();
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{} did not provide stderr.", program.display()));
        }
    };
    let stdout_reader =
        spawn_bounded_reader(stdout, output_limit, "command-stdout").map_err(|error| {
            let _ = job.terminate();
            let _ = child.kill();
            let _ = child.wait();
            format!("{} stdout could not be drained: {error}", program.display())
        })?;
    let stderr_reader = match spawn_bounded_reader(stderr, output_limit, "command-stderr") {
        Ok(reader) => reader,
        Err(error) => {
            let _ = job.terminate();
            let _ = child.kill();
            let _ = child.wait();
            drop(job);
            let _ = stdout_reader.join();
            return Err(format!(
                "{} stderr could not be drained: {error}",
                program.display()
            ));
        }
    };

    let started = Instant::now();
    let completion = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Ok(None) => {
                let tree_error = job.terminate().err();
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "{} timed out after {} seconds.{}",
                    program.display(),
                    timeout.as_secs(),
                    tree_error
                        .map(|error| format!(" Process-tree cleanup also failed: {error}"))
                        .unwrap_or_default()
                ));
            }
            Err(error) => {
                let tree_error = job.terminate().err();
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "{} could not be monitored: {error}.{}",
                    program.display(),
                    tree_error
                        .map(|error| format!(" Process-tree cleanup also failed: {error}"))
                        .unwrap_or_default()
                ));
            }
        }
    };
    // A successful direct child must not leave detached descendants holding
    // stdout/stderr open. Terminating the job before joining the drainers makes
    // EOF deterministic and closes the same process tree on every exit path.
    let job_completion = job.terminate().map_err(|error| error.to_string());
    drop(job);
    // Join both drainers even if one failed so no reader thread is detached.
    let stdout = join_bounded_reader(stdout_reader, "stdout");
    let stderr = join_bounded_reader(stderr_reader, "stderr");
    let stdout = stdout?;
    let stderr = stderr?;
    job_completion?;
    let text = combine_decoded_output(&stdout.bytes, &stderr.bytes, encoding)?;
    if stdout.truncated || stderr.truncated {
        return Err(format!(
            "{} produced more than {} bytes on a captured stream; output was rejected instead of using a truncated result.",
            program.display(),
            output_limit
        ));
    }
    let status = match completion {
        Ok(status) => status,
        Err(error) if text.is_empty() => return Err(error),
        Err(error) => return Err(format!("{error} Output: {text}")),
    };
    if status.success() {
        Ok(text)
    } else {
        Err(format!(
            "{} exited with {}: {text}",
            program.display(),
            status
        ))
    }
}

fn spawn_bounded_reader<R>(
    reader: R,
    limit: usize,
    name: &str,
) -> std::io::Result<JoinHandle<std::io::Result<BoundedOutput>>>
where
    R: Read + Send + 'static,
{
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || read_bounded(reader, limit))
}

fn read_bounded<R: Read>(mut reader: R, limit: usize) -> std::io::Result<BoundedOutput> {
    let mut bytes = Vec::with_capacity(limit.min(16 * 1024));
    let mut buffer = [0_u8; 16 * 1024];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let accepted = limit.saturating_sub(bytes.len()).min(count);
        bytes.extend_from_slice(&buffer[..accepted]);
        truncated |= accepted < count;
    }
    Ok(BoundedOutput { bytes, truncated })
}

fn join_bounded_reader(
    reader: JoinHandle<std::io::Result<BoundedOutput>>,
    stream: &str,
) -> Result<BoundedOutput, String> {
    reader
        .join()
        .map_err(|_| format!("The {stream} reader thread panicked."))?
        .map_err(|error| format!("The {stream} stream could not be read: {error}"))
}

fn combine_decoded_output(
    stdout: &[u8],
    stderr: &[u8],
    encoding: OutputEncoding,
) -> Result<String, String> {
    let stdout = decode_output(stdout, encoding)?;
    let stderr = decode_output(stderr, encoding)?;
    Ok(match (stdout.trim(), stderr.trim()) {
        ("", "") => String::new(),
        (stdout, "") => stdout.to_owned(),
        ("", stderr) => stderr.to_owned(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    })
}

fn decode_output(bytes: &[u8], encoding: OutputEncoding) -> Result<String, String> {
    match encoding {
        OutputEncoding::Utf8Lossy => Ok(String::from_utf8_lossy(bytes).into_owned()),
        OutputEncoding::Utf16Le => decode_utf16_le(bytes),
    }
}

fn decode_utf16_le(bytes: &[u8]) -> Result<String, String> {
    let bytes = bytes.strip_prefix(&[0xff, 0xfe]).unwrap_or(bytes);
    if !bytes.len().is_multiple_of(2) {
        return Err("A command returned an odd number of UTF-16LE bytes.".into());
    }
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    String::from_utf16(&units)
        .map_err(|error| format!("A command returned invalid UTF-16LE output: {error}"))
}

fn parse_aseprite_version(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|part| {
        let token = part.trim_matches(|character: char| {
            !character.is_ascii_alphanumeric() && character != '.' && character != '-'
        });
        let value = token.strip_prefix('v').unwrap_or(token);
        let (numbers, prerelease) = value
            .split_once('-')
            .map_or((value, None), |(numbers, suffix)| (numbers, Some(suffix)));
        let components = numbers.split('.').collect::<Vec<_>>();
        if !(2..=4).contains(&components.len())
            || components[0] != "1"
            || components[1] != "3"
            || components.iter().any(|component| {
                component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit())
            })
        {
            return None;
        }
        if prerelease.is_some_and(|suffix| {
            ["alpha", "beta", "rc"].iter().all(|prefix| {
                !suffix.strip_prefix(prefix).is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                })
            })
        }) {
            return None;
        }
        Some(format!("v{value}"))
    })
}

fn managed_fingerprint_matches(record: &crate::models::ManagedRecord) -> bool {
    let Some(expected) = record.bundle_fingerprint.as_deref() else {
        return false;
    };
    artifact_fingerprint(Path::new(&record.path))
        .map(|actual| hex::encode(actual).eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ManagedRecord;
    use std::io::Cursor;
    use std::os::windows::fs::symlink_file;
    use tempfile::tempdir;

    fn write_x64_pe(path: &Path) {
        let mut bytes = vec![0_u8; 70];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64_u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&0x8664_u16.to_le_bytes());
        std::fs::write(path, bytes).unwrap();
    }

    fn managed_record(path: String) -> ManagedRecord {
        ManagedRecord {
            id: "persisted-installation-id".into(),
            path,
            tag: "v1.3.18.1".into(),
            source_version: Some("v1.3.18.1".into()),
            version_exact: true,
            digest: "digest".into(),
            architecture: "x86_64".into(),
            installed_at: "2026-08-01T00:00:00Z".into(),
            bundle_fingerprint: None,
            backup_path: None,
            backup_tag: None,
            backup_source_version: None,
            backup_digest: None,
            backup_installed_at: None,
            backup_version_exact: None,
            backup_bundle_fingerprint: None,
            backup_architecture: None,
            integration_paths: Vec::new(),
        }
    }

    #[test]
    fn installation_identity_ignores_extended_path_prefix_and_case() {
        assert_eq!(
            installation_id(r"C:\Users\Example\Aseprite"),
            installation_id(r"\\?\c:/users/example/ASEPRITE\")
        );
        assert_eq!(
            installation_id(r"\\server\share\Aseprite"),
            installation_id(r"\\?\UNC\SERVER\SHARE\aseprite\")
        );
    }

    #[test]
    fn managed_discovery_preserves_persisted_id_and_visible_path() {
        let directory = tempdir().unwrap();
        write_x64_pe(&directory.path().join("aseprite.exe"));
        std::fs::create_dir(directory.path().join("data")).unwrap();
        std::fs::write(directory.path().join("icudtl.dat"), b"icu").unwrap();
        let visible_path = directory.path().to_string_lossy().into_owned();
        let mut record = managed_record(visible_path.clone());
        record.bundle_fingerprint =
            Some(hex::encode(artifact_fingerprint(directory.path()).unwrap()));
        let state = ManagedState {
            schema_version: 2,
            installations: vec![record],
        };

        let installation =
            inspect_candidate(directory.path(), InstallationChannel::Manual, &state).unwrap();
        assert_eq!(installation.id, "persisted-installation-id");
        assert_eq!(installation.path, visible_path);
        assert_eq!(installation.channel, InstallationChannel::Managed);
    }

    #[test]
    fn discovery_does_not_trust_a_managed_record_without_its_fingerprint() {
        let directory = tempdir().unwrap();
        write_x64_pe(&directory.path().join("aseprite.exe"));
        std::fs::create_dir(directory.path().join("data")).unwrap();
        std::fs::write(directory.path().join("icudtl.dat"), b"icu").unwrap();
        let visible_path = directory.path().to_string_lossy().into_owned();
        let state = ManagedState {
            schema_version: 2,
            installations: vec![managed_record(visible_path)],
        };

        let installation =
            inspect_candidate(directory.path(), InstallationChannel::Manual, &state).unwrap();
        assert_eq!(installation.channel, InstallationChannel::Manual);
        assert_eq!(installation.version, None);
        assert!(!installation.version_exact);
    }

    #[test]
    fn version_parser_requires_an_exact_supported_token() {
        assert_eq!(
            parse_aseprite_version("Aseprite v1.3.18.1\r\n"),
            Some("v1.3.18.1".into())
        );
        assert_eq!(
            parse_aseprite_version("Aseprite v1.3.18-beta2"),
            Some("v1.3.18-beta2".into())
        );
        assert_eq!(parse_aseprite_version("Aseprite v1.3foo"), None);
        assert_ne!(
            parse_aseprite_version("Aseprite v1.3.18.1"),
            parse_aseprite_version("Aseprite v1.3.1")
        );
    }

    #[test]
    fn bounded_reader_drains_but_never_keeps_more_than_limit() {
        let output = read_bounded(Cursor::new(vec![b'x'; 128 * 1024]), 1024).unwrap();
        assert_eq!(output.bytes.len(), 1024);
        assert!(output.truncated);
        assert!(output.bytes.iter().all(|byte| *byte == b'x'));
    }

    #[test]
    fn utf16le_decoder_handles_bom_and_non_ascii_environment() {
        let text = "PATH=C:\\Program Files\\工具\r\nINCLUDE=C:\\SDK\r\n";
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let decoded = decode_utf16_le(&bytes).unwrap();
        let environment = parse_vc_environment(&decoded);
        assert_eq!(
            environment.get(OsStr::new("PATH")),
            Some(&OsString::from(r"C:\Program Files\工具"))
        );
        assert!(decode_utf16_le(&[0x41]).is_err());
        assert!(decode_utf16_le(&[0x00, 0xd8]).is_err());
    }

    #[test]
    fn fingerprint_is_stable_and_uses_unambiguous_entry_framing() {
        let first = tempdir().unwrap();
        std::fs::write(first.path().join("a"), b"bc").unwrap();
        let first_fingerprint = artifact_fingerprint(first.path()).unwrap();
        assert_eq!(
            first_fingerprint,
            artifact_fingerprint(first.path()).unwrap()
        );

        let second = tempdir().unwrap();
        std::fs::write(second.path().join("ab"), b"c").unwrap();
        assert_ne!(
            first_fingerprint,
            artifact_fingerprint(second.path()).unwrap()
        );
    }

    #[test]
    fn fingerprint_rejects_reparse_points_before_artifact_launch() {
        let directory = tempdir().unwrap();
        std::fs::create_dir(directory.path().join("data")).unwrap();
        write_x64_pe(&directory.path().join("aseprite.exe"));
        std::fs::write(directory.path().join("icudtl.dat"), b"icu").unwrap();
        let target = directory.path().join("target.txt");
        std::fs::write(&target, b"target").unwrap();
        let link = directory.path().join("data/link.txt");
        if let Err(error) = symlink_file(&target, &link) {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                return;
            }
            panic!("could not create test symlink: {error}");
        }

        let error = validate_artifact(directory.path(), "v1.3.18.1").unwrap_err();
        assert_eq!(error.code, "artifactReparsePoint");
    }

    #[test]
    fn windows_volume_keys_normalize_verbatim_drive_prefixes() {
        assert_eq!(
            windows_volume_identity(Path::new(r"C:\cache")),
            windows_volume_identity(Path::new(r"\\?\C:\cache"))
        );
    }

    #[test]
    fn unknown_elevation_is_a_blocking_prerequisite() {
        let (ok, detail, remediation) = standard_user_prerequisite(Err("probe failed".into()));
        assert!(!ok);
        assert!(detail.contains("probe failed"));
        assert!(remediation.is_some());
    }

    #[test]
    fn unc_detection_distinguishes_network_and_verbatim_local_paths() {
        assert!(path_is_unc(Path::new(r"\\server\share\Aseprite")));
        assert!(path_is_unc(Path::new(r"\\?\UNC\server\share\Aseprite")));
        assert!(!path_is_unc(Path::new(r"C:\Users\Example\Aseprite")));
        assert!(!path_is_unc(Path::new(r"\\?\C:\Users\Example\Aseprite")));
    }

    #[test]
    fn workspace_probe_takes_the_operation_lock_exclusively() {
        let directory = tempdir().unwrap();
        let paths = InstallerPaths::new(
            directory.path().join("data"),
            directory.path().join("cache"),
        );
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        let lock_path = paths.data_dir.join(".operation.lock");
        let held = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        held.try_lock_exclusive().unwrap();

        let error = probe_workspace(
            &paths,
            &directory.path().join("destination/Aseprite"),
            false,
        )
        .unwrap_err();
        assert!(error.contains("already locked"));
        FileExt::unlock(&held).unwrap();
    }

    #[test]
    fn steam_parser_accepts_only_absolute_local_library_paths() {
        let paths = parse_steam_library_paths(
            r#"        "path"        "D:\\SteamLibrary"
        "path"        "relative\\SteamLibrary"
        "path"        "\\\\server\\share""#,
        );
        assert_eq!(paths, vec![PathBuf::from(r"D:\SteamLibrary")]);
    }

    #[test]
    fn registry_paths_accept_bounded_local_locations_without_command_arguments() {
        assert_eq!(
            registry_local_path(r#""C:\Users\Example\Aseprite\aseprite.exe",0"#, true),
            Some(PathBuf::from(r"C:\Users\Example\Aseprite\aseprite.exe"))
        );
        assert_eq!(
            registry_local_path(r"C:\Program Files\Aseprite", false),
            Some(PathBuf::from(r"C:\Program Files\Aseprite"))
        );
        assert_eq!(
            registry_local_path(r#""C:\Aseprite\aseprite.exe" --unsafe"#, true),
            None
        );
        assert_eq!(
            registry_local_path(r"\\server\share\aseprite.exe", true),
            None
        );
    }

    #[test]
    fn writable_discovery_uses_directory_access_rights() {
        let directory = tempdir().unwrap();
        assert!(directory_is_probably_writable(directory.path()));
    }

    #[test]
    fn program_files_candidates_remain_external_and_non_manageable() {
        let Some(program_files) = std::env::var_os("ProgramFiles") else {
            return;
        };
        let candidate = PathBuf::from(program_files).join("Aseprite");
        assert_eq!(
            infer_channel(&candidate, InstallationChannel::Manual),
            InstallationChannel::PackageManager
        );
        assert!(!directory_is_probably_writable(&candidate));
    }

    #[test]
    fn shortcut_names_are_install_specific_and_traversal_free() {
        let id = "windows-0123456789abcdef";
        let path = PathBuf::from(format!(
            r"C:\Users\Example\Programs\{SHORTCUT_PREFIX}{id}{SHORTCUT_SUFFIX}"
        ));
        assert_eq!(shortcut_installation_id(&path), Some(id));
        assert!(shortcut_installation_id(Path::new(
            r"C:\Users\Example\Programs\Aseprite (Local Build).lnk"
        ))
        .is_none());
        assert!(validate_integration_id(r"..\foreign").is_err());
    }
}
