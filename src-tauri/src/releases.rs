use crate::error::{AppResult, InstallerError};
use crate::models::ReleaseInfo;
use reqwest::header::{ETAG, IF_NONE_MATCH};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;
use url::Url;

const RELEASES_URL: &str = "https://api.github.com/repos/aseprite/aseprite/releases?per_page=100";
const RELEASE_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

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
    let releases =
        list_releases_from_url(client, cache_dir, include_prereleases, RELEASES_URL).await?;
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let releases = {
        let mut releases = releases;
        releases.retain(|release| portable_source_supported(&release.source_asset_name));
        if releases.is_empty() {
            return Err(InstallerError::new(
                "noPortableReleases",
                "No Aseprite source release with a pinned compatible Skia toolchain is available for this platform.",
            ));
        }
        for release in &mut releases {
            release.latest = false;
        }
        if let Some(latest) = releases.iter_mut().find(|release| !release.prerelease) {
            latest.latest = true;
        }
        releases
    };
    Ok(releases)
}

async fn list_releases_from_url(
    client: &reqwest::Client,
    cache_dir: &Path,
    include_prereleases: bool,
    releases_url: &str,
) -> AppResult<Vec<ReleaseInfo>> {
    let cache_setup_error = std::fs::create_dir_all(cache_dir).err();
    let json_path = cache_dir.join("releases.json");
    let etag_path = cache_dir.join("releases.etag");
    let (cached_json, cache_read_error) = match std::fs::read_to_string(&json_path) {
        Ok(json) => (Some(json), None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, None),
        Err(error) => (None, Some(error)),
    };

    let mut request = client.get(releases_url).timeout(RELEASE_REQUEST_TIMEOUT);
    if let Ok(etag) = std::fs::read_to_string(&etag_path) {
        if !etag.trim().is_empty() {
            request = request.header(IF_NONE_MATCH, etag.trim());
        }
    }

    let json = match request.send().await {
        Ok(response) if response.status() == reqwest::StatusCode::NOT_MODIFIED => cached_json
            .ok_or_else(|| {
                cache_unavailable_error(
                    "GitHub returned cached release data, but the local cache cannot be read. Restore read/write access to the installer cache (do not run the installer with sudo), then try again.",
                    cache_setup_error.as_ref(),
                    cache_read_error.as_ref(),
                )
            })?,
        Ok(response) if response.status().is_success() => {
            let etag = response
                .headers()
                .get(ETAG)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            match response.text().await {
                Ok(body) => {
                    // Release data is usable even when a root-owned or read-only
                    // cache cannot be refreshed. The workspace preflight will
                    // report that storage blocker with its remediation.
                    if std::fs::write(&json_path, &body).is_ok() {
                        if let Some(etag) = etag {
                            let _ = std::fs::write(&etag_path, etag);
                        }
                    }
                    body
                }
                Err(error) => cached_json.ok_or_else(|| github_network_error(error))?,
            }
        }
        Ok(response) => cached_json.ok_or_else(|| github_http_error(response.status()))?,
        Err(error) => cached_json.ok_or_else(|| {
            append_cache_context(
                github_network_error(error),
                cache_setup_error.as_ref(),
                cache_read_error.as_ref(),
            )
        })?,
    };

    parse_releases(&json, include_prereleases)
}

fn github_network_error(error: reqwest::Error) -> InstallerError {
    let (code, message) = if error.is_timeout() {
        (
            "networkTimeout",
            "GitHub stopped responding before the release data arrived. Check your connection, VPN, system proxy, or security software, then try again.",
        )
    } else if error.is_connect() {
        (
            "networkConnection",
            "A secure connection to GitHub could not be established. Check DNS, VPN or proxy settings and, on a managed device, the corporate TLS certificate trust.",
        )
    } else if error.is_body() || error.is_decode() {
        (
            "networkRead",
            "The GitHub release response was interrupted. Check the proxy, VPN, or security software, then try again.",
        )
    } else if error.is_redirect() {
        (
            "networkRedirect",
            "GitHub redirects could not be followed safely. Check whether a proxy or security filter is rewriting the request.",
        )
    } else {
        (
            "network",
            "The official Aseprite release data could not be downloaded. Check the network, proxy, VPN, or security software, then try again.",
        )
    };
    InstallerError::with_detail(code, message, error.to_string())
}

fn github_http_error(status: reqwest::StatusCode) -> InstallerError {
    let (code, message) = if status == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        (
            "proxyAuthentication",
            "The configured proxy requires authentication before GitHub can be reached. Sign in to the proxy or ask your administrator to allow api.github.com.",
        )
    } else if status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        (
            "githubAccess",
            "GitHub refused the release-data request. Wait for any rate limit to reset, or check whether a proxy or security policy blocks api.github.com.",
        )
    } else if status.is_server_error() {
        (
            "githubUnavailable",
            "GitHub is temporarily unavailable. Try again later.",
        )
    } else {
        (
            "github",
            "GitHub release data is unavailable. Check whether the network or proxy allows api.github.com.",
        )
    };
    InstallerError::with_detail(code, message, format!("HTTP {status}"))
}

fn cache_unavailable_error(
    message: &str,
    setup_error: Option<&std::io::Error>,
    read_error: Option<&std::io::Error>,
) -> InstallerError {
    append_cache_context(
        InstallerError::new("releaseCache", message),
        setup_error,
        read_error,
    )
}

fn append_cache_context(
    mut error: InstallerError,
    setup_error: Option<&std::io::Error>,
    read_error: Option<&std::io::Error>,
) -> InstallerError {
    let cache_detail = read_error
        .or(setup_error)
        .map(|cache_error| format!("The local release cache is also unavailable: {cache_error}"));
    if let Some(cache_detail) = cache_detail {
        error.detail = Some(match error.detail.take() {
            Some(detail) => format!("{detail}. {cache_detail}"),
            None => cache_detail,
        });
    }
    error
}

fn parse_releases(json: &str, include_prereleases: bool) -> AppResult<Vec<ReleaseInfo>> {
    let releases: Vec<GitHubRelease> = serde_json::from_str(json).map_err(|error| {
        InstallerError::with_detail(
            "githubData",
            "GitHub returned invalid release data.",
            error.to_string(),
        )
    })?;

    let mut parsed = releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let tag_version = parse_supported_version(&release.tag_name)?;
            let asset = release.assets.into_iter().find(valid_source_asset)?;
            let source_version = source_build_requirements(&asset.name)?;
            let source_version = parse_supported_version(source_version.source_version)?;
            let prerelease =
                release.prerelease || tag_version.is_prerelease() || source_version.is_prerelease();
            if prerelease && !include_prereleases {
                return None;
            }
            let digest = asset.digest?;
            Some(ReleaseInfo {
                latest: false,
                tag: release.tag_name.clone(),
                name: release
                    .name
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| format!("Aseprite {}", release.tag_name)),
                published_at: release.published_at.unwrap_or_default(),
                prerelease,
                source_asset_name: asset.name,
                source_url: asset.browser_download_url,
                digest,
                size: asset.size,
            })
        })
        .collect::<Vec<_>>();

    parsed.sort_by_key(|release| std::cmp::Reverse(source_version_key(release)));
    if let Some(latest) = parsed.iter_mut().find(|release| !release.prerelease) {
        latest.latest = true;
    }

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
    prerelease_rank: u8,
    prerelease_number: u64,
}

impl VersionKey {
    fn is_prerelease(self) -> bool {
        self.prerelease_rank < 3
    }
}

const MINIMUM_SUPPORTED_SOURCE: [u64; 4] = [1, 3, 15, 5];
#[cfg(any(target_os = "linux", target_os = "windows", test))]
const MAXIMUM_PORTABLE_M124_SOURCE: [u64; 4] = [1, 3, 18, 1];
const CMAKE_3_20: [u64; 3] = [3, 20, 0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceBuildRequirements<'a> {
    pub source_version: &'a str,
    pub minimum_cmake_version: [u64; 3],
}

pub(crate) fn source_build_requirements(
    source_asset_name: &str,
) -> Option<SourceBuildRequirements<'_>> {
    let source_version = source_asset_name
        .strip_prefix("Aseprite-")?
        .strip_suffix("-Source.zip")?;
    let version = parse_supported_version(source_version)?;
    if version.numbers < MINIMUM_SUPPORTED_SOURCE {
        return None;
    }

    Some(SourceBuildRequirements {
        source_version,
        minimum_cmake_version: CMAKE_3_20,
    })
}

#[cfg(any(target_os = "linux", target_os = "windows", test))]
pub(crate) fn portable_source_supported(source_asset_name: &str) -> bool {
    source_build_requirements(source_asset_name)
        .and_then(|requirements| parse_supported_version(requirements.source_version))
        .is_some_and(|version| version.numbers <= MAXIMUM_PORTABLE_M124_SOURCE)
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
    let (prerelease_rank, prerelease_number) = match prerelease {
        None => (3, 0),
        Some(suffix) => {
            let (rank, number) = [("rc", 2_u8), ("beta", 1_u8), ("alpha", 0_u8)]
                .into_iter()
                .find_map(|(prefix, rank)| {
                    suffix.strip_prefix(prefix).and_then(|number| {
                        (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
                            .then(|| number.parse::<u64>().ok().map(|number| (rank, number)))
                            .flatten()
                    })
                })?;
            (rank, number)
        }
    };
    Some(VersionKey {
        numbers,
        prerelease_rank,
        prerelease_number,
    })
}

fn valid_source_asset(asset: &GitHubAsset) -> bool {
    if asset.size == 0 || source_build_requirements(&asset.name).is_none() {
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

fn source_version_key(release: &ReleaseInfo) -> Option<VersionKey> {
    source_build_requirements(&release.source_asset_name)
        .and_then(|requirements| parse_supported_version(requirements.source_version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::thread;

    fn release_fixture() -> String {
        let digest = format!("sha256:{}", "a".repeat(64));
        serde_json::json!([{
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
        }])
        .to_string()
    }

    #[test]
    fn portable_manifest_stops_before_an_unpinned_skia_revision() {
        assert!(portable_source_supported("Aseprite-v1.3.15.5-Source.zip"));
        assert!(portable_source_supported("Aseprite-v1.3.18.1-Source.zip"));
        assert!(!portable_source_supported("Aseprite-v1.3.18.2-Source.zip"));
        assert!(!portable_source_supported("Aseprite-v1.3.19-Source.zip"));
    }

    fn serve_once(body: Option<String>, delay: Duration) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2_048];
            let _ = stream.read(&mut request);
            thread::sleep(delay);
            if let Some(body) = body {
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://{address}/releases"), server)
    }

    fn test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

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
    fn classifies_prereleases_from_the_actual_source_even_when_github_does_not() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let fixture = serde_json::json!([{
            "tag_name": "v1.3.16-beta2",
            "name": "Aseprite v1.3.16-beta3",
            "published_at": "2025-08-21T00:00:00Z",
            "prerelease": false,
            "draft": false,
            "assets": [{
                "name": "Aseprite-v1.3.16-beta3-Source.zip",
                "browser_download_url": "https://github.com/aseprite/aseprite/releases/download/v1.3.16-beta2/Aseprite-v1.3.16-beta3-Source.zip",
                "digest": digest,
                "size": 10
            }]
        }]);

        assert_eq!(
            parse_releases(&fixture.to_string(), false)
                .unwrap_err()
                .code,
            "noReleases"
        );
        let releases = parse_releases(&fixture.to_string(), true).unwrap();
        assert!(releases[0].prerelease);
        assert!(!releases[0].latest);
    }

    #[test]
    fn source_suffix_alone_marks_a_release_as_prerelease() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let fixture = serde_json::json!([{
            "tag_name": "v1.3.16",
            "name": "Aseprite v1.3.16",
            "published_at": "2025-08-21T00:00:00Z",
            "prerelease": false,
            "draft": false,
            "assets": [{
                "name": "Aseprite-v1.3.16-beta3-Source.zip",
                "browser_download_url": "https://github.com/aseprite/aseprite/releases/download/v1.3.16/Aseprite-v1.3.16-beta3-Source.zip",
                "digest": digest,
                "size": 10
            }]
        }]);

        assert_eq!(
            parse_releases(&fixture.to_string(), false)
                .unwrap_err()
                .code,
            "noReleases"
        );
        let releases = parse_releases(&fixture.to_string(), true).unwrap();
        assert!(releases[0].prerelease);
        assert!(!releases[0].latest);
    }

    #[test]
    fn sorts_and_marks_latest_by_source_versions_not_mismatched_release_tags() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let release = |tag: &str, source: &str, prerelease: bool| {
            serde_json::json!({
                "tag_name": tag,
                "name": tag,
                "published_at": "2026-01-01T00:00:00Z",
                "prerelease": prerelease,
                "draft": false,
                "assets": [{
                    "name": format!("Aseprite-{source}-Source.zip"),
                    "browser_download_url": format!("https://github.com/aseprite/aseprite/releases/download/{tag}/Aseprite-{source}-Source.zip"),
                    "digest": digest,
                    "size": 10
                }]
            })
        };
        let fixture = serde_json::Value::Array(vec![
            release("v1.3.15.4", "v1.3.15.5", false),
            release("v1.3.19-beta2", "v1.3.19-beta10", true),
            release("v1.3.19-beta9", "v1.3.19-beta2", true),
            release("v1.3.19-rc2", "v1.3.19-rc2", true),
            release("v1.3.19", "v1.3.19", false),
        ]);

        let releases = parse_releases(&fixture.to_string(), true).unwrap();
        assert_eq!(
            releases
                .iter()
                .map(|release| release.tag.as_str())
                .collect::<Vec<_>>(),
            vec![
                "v1.3.19",
                "v1.3.19-rc2",
                "v1.3.19-beta2",
                "v1.3.19-beta9",
                "v1.3.15.4"
            ]
        );
        assert!(releases[0].latest);
        assert!(releases.iter().skip(1).all(|release| !release.latest));
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

    #[test]
    fn derives_requirements_from_the_actual_source_asset_version() {
        for (asset_name, source_version, minimum_cmake_version) in [
            ("Aseprite-v1.3.15.5-Source.zip", "v1.3.15.5", CMAKE_3_20),
            ("Aseprite-v1.3.16.1-Source.zip", "v1.3.16.1", CMAKE_3_20),
        ] {
            assert_eq!(
                source_build_requirements(asset_name),
                Some(SourceBuildRequirements {
                    source_version,
                    minimum_cmake_version,
                })
            );
        }
    }

    #[test]
    fn preserves_prerelease_suffixes_in_source_versions() {
        assert_eq!(
            source_build_requirements("Aseprite-v1.3.16-beta3-Source.zip"),
            Some(SourceBuildRequirements {
                source_version: "v1.3.16-beta3",
                minimum_cmake_version: CMAKE_3_20,
            })
        );
    }

    #[test]
    fn rejects_malformed_or_unsupported_source_asset_names() {
        for asset_name in [
            "Aseprite-v1.2.40-Source.zip",
            "Aseprite-v1.3.14.4-Source.zip",
            "Aseprite-v1.3.15.4-Source.zip",
            "Aseprite-v1.3.invalid-Source.zip",
            "v1.3.18.1-Source.zip",
            "Aseprite-v1.3.18.1.zip",
        ] {
            assert!(
                source_build_requirements(asset_name).is_none(),
                "{asset_name}"
            );
        }
    }

    #[test]
    fn release_fetch_succeeds_when_the_cache_path_is_not_writable_storage() {
        let directory = tempfile::tempdir().unwrap();
        let cache_path = directory.path().join("cache-is-a-file");
        std::fs::write(&cache_path, b"not a directory").unwrap();
        let (url, server) = serve_once(Some(release_fixture()), Duration::ZERO);
        let client = reqwest::Client::builder().no_proxy().build().unwrap();

        let releases = test_runtime()
            .block_on(list_releases_from_url(&client, &cache_path, false, &url))
            .unwrap();

        server.join().unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].tag, "v1.3.18.1");
        assert!(cache_path.is_file());
    }

    #[test]
    fn stalled_release_headers_return_an_actionable_timeout() {
        let directory = tempfile::tempdir().unwrap();
        let cache_path = directory.path().join("cache");
        let (url, server) = serve_once(None, Duration::from_millis(250));
        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(1))
            .read_timeout(Duration::from_millis(40))
            .build()
            .unwrap();

        let error = test_runtime()
            .block_on(list_releases_from_url(&client, &cache_path, false, &url))
            .unwrap_err();

        server.join().unwrap();
        assert_eq!(error.code, "networkTimeout");
        assert!(error.message.contains("proxy"));
    }

    #[test]
    fn proxy_authentication_errors_are_actionable() {
        let error = github_http_error(reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED);
        assert_eq!(error.code, "proxyAuthentication");
        assert!(error.message.contains("proxy"));
        assert!(error.message.contains("api.github.com"));
    }
}
