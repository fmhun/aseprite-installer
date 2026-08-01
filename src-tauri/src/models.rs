use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    pub tag: String,
    pub name: String,
    pub published_at: String,
    pub prerelease: bool,
    pub latest: bool,
    pub source_asset_name: String,
    pub source_url: String,
    pub digest: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InstallationChannel {
    Managed,
    Manual,
    Steam,
    PackageManager,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationInfo {
    pub id: String,
    pub path: String,
    pub version: Option<String>,
    pub version_exact: bool,
    pub architecture: Option<String>,
    pub channel: InstallationChannel,
    pub manageable: bool,
    pub writable: bool,
    pub has_backup: bool,
    pub installed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prerequisite {
    pub id: String,
    pub label: String,
    pub ok: bool,
    pub required: bool,
    pub detail: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    pub ready: bool,
    pub architecture: String,
    pub os_version: String,
    pub free_bytes: u64,
    pub minimum_free_bytes: u64,
    pub homebrew_available: bool,
    pub prerequisites: Vec<Prerequisite>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OperationStage {
    Idle,
    Preflight,
    Downloading,
    Verifying,
    Extracting,
    Compiling,
    Signing,
    BackingUp,
    Installing,
    Finalizing,
    Validating,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationProgress {
    pub stage: OperationStage,
    pub percent: Option<u8>,
    pub message: String,
    pub log_line: Option<String>,
}

impl OperationProgress {
    pub fn stage(stage: OperationStage, percent: Option<u8>, message: impl Into<String>) -> Self {
        Self {
            stage,
            percent,
            message: message.into(),
            log_line: None,
        }
    }

    pub fn log(stage: OperationStage, line: impl Into<String>) -> Self {
        Self {
            stage,
            percent: None,
            message: "Compiling Aseprite…".into(),
            log_line: Some(line.into()),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRequest {
    pub tag: String,
    pub target_path: Option<String>,
    pub adopt: bool,
    pub eula_accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRecord {
    pub id: String,
    pub path: String,
    pub tag: String,
    #[serde(default)]
    pub source_version: Option<String>,
    #[serde(default = "default_true")]
    pub version_exact: bool,
    pub digest: String,
    pub architecture: String,
    pub installed_at: String,
    #[serde(default)]
    pub bundle_fingerprint: Option<String>,
    pub backup_path: Option<String>,
    #[serde(default)]
    pub backup_tag: Option<String>,
    #[serde(default)]
    pub backup_source_version: Option<String>,
    #[serde(default)]
    pub backup_digest: Option<String>,
    #[serde(default)]
    pub backup_installed_at: Option<String>,
    #[serde(default)]
    pub backup_version_exact: Option<bool>,
    #[serde(default)]
    pub backup_bundle_fingerprint: Option<String>,
    #[serde(default)]
    pub backup_architecture: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedState {
    pub schema_version: u32,
    pub installations: Vec<ManagedRecord>,
}

impl Default for ManagedState {
    fn default() -> Self {
        Self {
            schema_version: 2,
            installations: Vec::new(),
        }
    }
}
