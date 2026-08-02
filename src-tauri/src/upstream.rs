use crate::error::{AppResult, InstallerError};
use serde::Deserialize;

pub(crate) const ASEPRITE_BUILD_SCRIPT: &str = "build.sh";
pub(crate) const ASEPRITE_BUILD_ARGUMENTS: [&str; 2] = ["--auto", "--norun"];
const ASEPRITE_DOCUMENTED_BUILD_OUTPUT: &str = "build/bin/Aseprite.app";
const INSTALLER_OUTPUT_POLICY: &str = "validated_aseprite_app_bundle_under_build_directory";
const COMPATIBILITY_MANIFEST_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../upstream/aseprite-compatibility.json"
));

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompatibilityContract {
    pub reviewed_through: String,
    pub baseline_tag: String,
    pub baseline_asset_name: String,
    pub baseline_asset_digest: String,
    pub baseline_asset_size: u64,
}

#[derive(Debug, Deserialize)]
struct CompatibilityManifest {
    schema_version: u32,
    recorded_at: String,
    upstream: UpstreamRepository,
    implementation_origin_commit: String,
    baseline_release: BaselineRelease,
    compatibility: CompatibilityPolicy,
    tracked_files: TrackedFiles,
}

#[derive(Debug, Deserialize)]
struct UpstreamRepository {
    repository: String,
    default_branch: String,
}

#[derive(Debug, Deserialize)]
struct BaselineRelease {
    release_id: u64,
    tag: String,
    commit: String,
    prerelease: bool,
    asset_id: u64,
    source_asset_name: String,
    source_asset_url: String,
    source_asset_digest: String,
    source_asset_size: u64,
}

#[derive(Debug, Deserialize)]
struct CompatibilityPolicy {
    series: String,
    reviewed_through: String,
    newer_release_policy: String,
    portable_release_policy: String,
}

#[derive(Debug, Deserialize)]
struct TrackedFiles {
    #[serde(rename = "INSTALL.md")]
    install: TrackedFile,
    #[serde(rename = "build.sh")]
    build: BuildContract,
}

#[derive(Debug, Deserialize)]
struct TrackedFile {
    baseline_blob_sha: String,
    observed_main_blob_sha: String,
    last_reviewed_change_commit: String,
    immutable_url: String,
    current_url: String,
}

#[derive(Debug, Deserialize)]
struct BuildContract {
    scope: String,
    #[serde(flatten)]
    file: TrackedFile,
    arguments: Vec<String>,
    documented_output: String,
    installer_output_policy: String,
}

pub(crate) fn compatibility_contract() -> AppResult<CompatibilityContract> {
    compatibility_contract_from_json(COMPATIBILITY_MANIFEST_JSON)
}

fn compatibility_contract_from_json(json: &str) -> AppResult<CompatibilityContract> {
    let manifest: CompatibilityManifest = serde_json::from_str(json).map_err(|error| {
        InstallerError::with_detail(
            "compatibilityData",
            "The bundled Aseprite compatibility policy is invalid.",
            error.to_string(),
        )
    })?;
    let invalid = |detail: &str| {
        InstallerError::with_detail(
            "compatibilityData",
            "The bundled Aseprite compatibility policy is invalid.",
            detail,
        )
    };

    if manifest.schema_version != 1 {
        return Err(invalid("Unsupported schema_version."));
    }
    if chrono::NaiveDate::parse_from_str(&manifest.recorded_at, "%Y-%m-%d").is_err() {
        return Err(invalid("recorded_at must be an ISO calendar date."));
    }
    if manifest.upstream.repository != "aseprite/aseprite"
        || manifest.upstream.default_branch != "main"
    {
        return Err(invalid("Unexpected upstream repository."));
    }
    if !valid_git_sha(&manifest.implementation_origin_commit)
        || !valid_git_sha(&manifest.baseline_release.commit)
    {
        return Err(invalid("Commit identities must be full Git SHA-1 values."));
    }
    if manifest.compatibility.series != "1.3"
        || manifest.compatibility.newer_release_policy != "blocked_until_review"
        || manifest.compatibility.portable_release_policy != "additional_pinned_skia_gate"
        || !stable_aseprite_13_tag(&manifest.compatibility.reviewed_through)
    {
        return Err(invalid("Unsupported compatibility policy."));
    }
    if manifest.baseline_release.tag != manifest.compatibility.reviewed_through {
        return Err(invalid(
            "baseline_release.tag must match compatibility.reviewed_through.",
        ));
    }

    let expected_asset_name = format!("Aseprite-{}-Source.zip", manifest.baseline_release.tag);
    let expected_asset_url = format!(
        "https://github.com/aseprite/aseprite/releases/download/{}/{}",
        manifest.baseline_release.tag, expected_asset_name
    );
    if manifest.baseline_release.release_id == 0
        || manifest.baseline_release.asset_id == 0
        || manifest.baseline_release.source_asset_size == 0
        || manifest.baseline_release.prerelease
        || manifest.baseline_release.source_asset_name != expected_asset_name
        || manifest.baseline_release.source_asset_url != expected_asset_url
        || !valid_sha256_digest(&manifest.baseline_release.source_asset_digest)
    {
        return Err(invalid("The baseline source asset identity is invalid."));
    }

    validate_tracked_file(
        "INSTALL.md",
        &manifest.tracked_files.install,
        &manifest.baseline_release.commit,
    )?;
    validate_tracked_file(
        ASEPRITE_BUILD_SCRIPT,
        &manifest.tracked_files.build.file,
        &manifest.baseline_release.commit,
    )?;
    if manifest.tracked_files.build.scope != "macos"
        || !manifest
            .tracked_files
            .build
            .arguments
            .iter()
            .map(String::as_str)
            .eq(ASEPRITE_BUILD_ARGUMENTS)
        || manifest.tracked_files.build.documented_output != ASEPRITE_DOCUMENTED_BUILD_OUTPUT
        || manifest.tracked_files.build.installer_output_policy != INSTALLER_OUTPUT_POLICY
    {
        return Err(invalid(
            "The recorded macOS build contract does not match the installer policy.",
        ));
    }

    Ok(CompatibilityContract {
        reviewed_through: manifest.compatibility.reviewed_through,
        baseline_tag: manifest.baseline_release.tag,
        baseline_asset_name: manifest.baseline_release.source_asset_name,
        baseline_asset_digest: manifest.baseline_release.source_asset_digest,
        baseline_asset_size: manifest.baseline_release.source_asset_size,
    })
}

fn validate_tracked_file(path: &str, file: &TrackedFile, baseline_commit: &str) -> AppResult<()> {
    if !valid_git_sha(&file.baseline_blob_sha)
        || !valid_git_sha(&file.observed_main_blob_sha)
        || !valid_git_sha(&file.last_reviewed_change_commit)
    {
        return Err(InstallerError::with_detail(
            "compatibilityData",
            "The bundled Aseprite compatibility policy is invalid.",
            format!("{path} contains an invalid Git identity."),
        ));
    }
    let expected_immutable_url =
        format!("https://github.com/aseprite/aseprite/blob/{baseline_commit}/{path}");
    let expected_current_url = format!("https://github.com/aseprite/aseprite/blob/main/{path}");
    if file.immutable_url != expected_immutable_url || file.current_url != expected_current_url {
        return Err(InstallerError::with_detail(
            "compatibilityData",
            "The bundled Aseprite compatibility policy is invalid.",
            format!("{path} contains an invalid upstream URL."),
        ));
    }
    Ok(())
}

fn stable_aseprite_13_tag(value: &str) -> bool {
    let Some(version) = value.strip_prefix('v') else {
        return false;
    };
    let parts = version.split('.').collect::<Vec<_>>();
    (2..=4).contains(&parts.len())
        && parts[0] == "1"
        && parts[1] == "3"
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn valid_git_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_contract_identifies_the_reviewed_baseline() {
        let contract = compatibility_contract().unwrap();
        assert_eq!(contract.reviewed_through, "v1.3.18.1");
        assert_eq!(contract.baseline_tag, "v1.3.18.1");
        assert!(valid_sha256_digest(&contract.baseline_asset_digest));
        assert!(contract.baseline_asset_size > 0);
    }

    #[test]
    fn rejects_a_manifest_whose_build_contract_diverges() {
        let mut manifest: serde_json::Value =
            serde_json::from_str(COMPATIBILITY_MANIFEST_JSON).unwrap();
        manifest["tracked_files"]["build.sh"]["arguments"] = serde_json::json!(["--auto"]);

        let error = compatibility_contract_from_json(&manifest.to_string()).unwrap_err();

        assert_eq!(error.code, "compatibilityData");
    }

    #[test]
    fn rejects_a_manifest_whose_boundary_and_baseline_diverge() {
        let mut manifest: serde_json::Value =
            serde_json::from_str(COMPATIBILITY_MANIFEST_JSON).unwrap();
        manifest["compatibility"]["reviewed_through"] = serde_json::json!("v1.3.17");

        let error = compatibility_contract_from_json(&manifest.to_string()).unwrap_err();

        assert_eq!(error.code, "compatibilityData");
    }
}
