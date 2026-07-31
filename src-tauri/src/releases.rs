use crate::error::{AppResult, InstallerError};
use crate::models::ReleaseInfo;
use reqwest::header::{ETAG, IF_NONE_MATCH};
use serde::Deserialize;
use std::path::Path;
use url::Url;

const RELEASES_URL: &str = "https://api.github.com/repos/aseprite/aseprite/releases?per_page=100";

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    name: Option<String>,
    published_at: Option<String>,
    prerelease: bool,
    draft: bool,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
    size: u64,
}

pub async fn list_releases(
    client: &reqwest::Client,
    cache_dir: &Path,
    include_prereleases: bool,
) -> AppResult<Vec<ReleaseInfo>> {
    std::fs::create_dir_all(cache_dir)?;
    let json_path = cache_dir.join("releases.json");
    let etag_path = cache_dir.join("releases.etag");

    let mut request = client.get(RELEASES_URL);
    if let Ok(etag) = std::fs::read_to_string(&etag_path) {
        if !etag.trim().is_empty() {
            request = request.header(IF_NONE_MATCH, etag.trim());
        }
    }

    let json = match request.send().await {
        Ok(response) if response.status() == reqwest::StatusCode::NOT_MODIFIED => {
            std::fs::read_to_string(&json_path).map_err(|error| {
                InstallerError::with_detail(
                    "releaseCache",
                    "GitHub returned cached release data, but the cache is missing.",
                    error.to_string(),
                )
            })?
        }
        Ok(response) if response.status().is_success() => {
            let etag = response
                .headers()
                .get(ETAG)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let body = response.text().await?;
            std::fs::write(&json_path, &body)?;
            if let Some(etag) = etag {
                std::fs::write(&etag_path, etag)?;
            }
            body
        }
        Ok(response) => {
            if json_path.exists() {
                std::fs::read_to_string(&json_path)?
            } else {
                return Err(InstallerError::with_detail(
                    "github",
                    "GitHub release data is unavailable.",
                    format!("HTTP {}", response.status()),
                ));
            }
        }
        Err(error) => {
            if json_path.exists() {
                std::fs::read_to_string(&json_path)?
            } else {
                return Err(error.into());
            }
        }
    };

    parse_releases(&json, include_prereleases)
}

fn parse_releases(json: &str, include_prereleases: bool) -> AppResult<Vec<ReleaseInfo>> {
    let releases: Vec<GitHubRelease> = serde_json::from_str(json).map_err(|error| {
        InstallerError::with_detail(
            "githubData",
            "GitHub returned invalid release data.",
            error.to_string(),
        )
    })?;

    let latest_stable = releases
        .iter()
        .filter(|release| !release.draft && !release.prerelease)
        .filter_map(|release| {
            parse_supported_version(&release.tag_name).map(|version| (version, release))
        })
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, release)| release.tag_name.clone());

    let mut parsed = releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter(|release| include_prereleases || !release.prerelease)
        .filter(|release| parse_supported_version(&release.tag_name).is_some())
        .filter_map(|release| {
            let asset = release.assets.into_iter().find(valid_source_asset)?;
            let digest = asset.digest?;
            Some(ReleaseInfo {
                latest: latest_stable.as_deref() == Some(&release.tag_name),
                tag: release.tag_name.clone(),
                name: release
                    .name
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| format!("Aseprite {}", release.tag_name)),
                published_at: release.published_at.unwrap_or_default(),
                prerelease: release.prerelease,
                source_asset_name: asset.name,
                source_url: asset.browser_download_url,
                digest,
                size: asset.size,
            })
        })
        .collect::<Vec<_>>();

    parsed.sort_by(|left, right| {
        parse_supported_version(&right.tag).cmp(&parse_supported_version(&left.tag))
    });

    if parsed.is_empty() {
        return Err(InstallerError::new(
            "noReleases",
            "No supported Aseprite 1.3 source releases with a SHA-256 digest were found.",
        ));
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct VersionKey {
    numbers: [u64; 4],
    stable: bool,
    prerelease_number: u64,
}

fn parse_supported_version(tag: &str) -> Option<VersionKey> {
    let version = tag.strip_prefix('v')?;
    let (main, prerelease) = version
        .split_once('-')
        .map_or((version, None), |(main, suffix)| (main, Some(suffix)));
    let parts = main.split('.').collect::<Vec<_>>();
    if parts.len() < 2 || parts.len() > 4 {
        return None;
    }
    let mut numbers = [0_u64; 4];
    for (index, part) in parts.into_iter().enumerate() {
        numbers[index] = part.parse().ok()?;
    }
    if numbers[0] != 1 || numbers[1] != 3 {
        return None;
    }
    let prerelease_number = prerelease
        .and_then(|suffix| {
            suffix
                .chars()
                .skip_while(|character| !character.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or(0);
    Some(VersionKey {
        numbers,
        stable: prerelease.is_none(),
        prerelease_number,
    })
}

fn valid_source_asset(asset: &GitHubAsset) -> bool {
    let expected_prefix = "Aseprite-v1.3";
    if !asset.name.starts_with(expected_prefix) || !asset.name.ends_with("-Source.zip") {
        return false;
    }
    let Some(digest) = asset.digest.as_deref() else {
        return false;
    };
    let Some(hex_digest) = digest.strip_prefix("sha256:") else {
        return false;
    };
    if hex_digest.len() != 64 || !hex_digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    let Ok(url) = Url::parse(&asset.browser_download_url) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url
            .path()
            .starts_with("/aseprite/aseprite/releases/download/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_drafts_legacy_versions_and_unverified_assets() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let fixture = serde_json::json!([
            {
                "tag_name": "v1.3.18.1",
                "name": "Aseprite v1.3.18.1",
                "published_at": "2026-07-23T21:23:49Z",
                "prerelease": false,
                "draft": false,
                "assets": [{
                    "name": "Aseprite-v1.3.18.1-Source.zip",
                    "browser_download_url": "https://github.com/aseprite/aseprite/releases/download/v1.3.18.1/Aseprite-v1.3.18.1-Source.zip",
                    "digest": digest,
                    "size": 10
                }]
            },
            {
                "tag_name": "v1.2.40",
                "name": "Legacy",
                "published_at": "2022-01-01T00:00:00Z",
                "prerelease": false,
                "draft": false,
                "assets": []
            }
        ]);
        let releases = parse_releases(&fixture.to_string(), false).unwrap();
        assert_eq!(releases.len(), 1);
        assert!(releases[0].latest);
        assert_eq!(releases[0].tag, "v1.3.18.1");
    }

    #[test]
    fn rejects_non_github_download_urls() {
        let asset = GitHubAsset {
            name: "Aseprite-v1.3.18.1-Source.zip".into(),
            browser_download_url: "https://example.com/source.zip".into(),
            digest: Some(format!("sha256:{}", "a".repeat(64))),
            size: 1,
        };
        assert!(!valid_source_asset(&asset));
    }
}
