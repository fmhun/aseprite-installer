#!/usr/bin/env node

import { appendFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

const API_VERSION = "2026-03-10";
const ISSUE_MARKER = "<!-- aseprite-upstream-watch:v1 -->";
const pageSize = 100;
const validateOnly = process.argv.includes("--validate-only");

function argument(name, fallback) {
  const index = process.argv.indexOf(name);
  return index >= 0 && process.argv[index + 1] ? process.argv[index + 1] : fallback;
}

function ensure(condition, message) {
  if (!condition) throw new Error(`Invalid compatibility manifest: ${message}`);
}

function isGitSha(value) {
  return typeof value === "string" && /^[0-9a-f]{40}$/i.test(value);
}

function isDigest(value) {
  return typeof value === "string" && /^sha256:[0-9a-f]{64}$/i.test(value);
}

function isPositiveInteger(value) {
  return Number.isSafeInteger(value) && value > 0;
}

function parseVersion(tag) {
  if (typeof tag !== "string" || !tag.startsWith("v")) return null;
  const [main, suffix] = tag.slice(1).split("-", 2);
  const parts = main.split(".");
  if (parts.length < 2 || parts.length > 4 || parts.some((part) => !/^\d+$/.test(part))) {
    return null;
  }
  const numbers = parts.map(Number);
  if (numbers.some((part) => !Number.isSafeInteger(part))) return null;
  while (numbers.length < 4) numbers.push(0);
  if (numbers[0] !== 1 || numbers[1] !== 3) return null;
  const prerelease = suffix?.match(/^(alpha|beta|rc)(\d+)$/);
  if (suffix !== undefined && !prerelease) return null;
  const prereleaseRank =
    suffix === undefined ? 3 : { alpha: 0, beta: 1, rc: 2 }[prerelease[1]];
  return {
    tag,
    numbers,
    stable: suffix === undefined,
    prereleaseRank,
    prereleaseNumber: prerelease ? Number(prerelease[2]) : 0,
  };
}

function compareVersions(left, right) {
  for (let index = 0; index < left.numbers.length; index += 1) {
    if (left.numbers[index] !== right.numbers[index]) {
      return left.numbers[index] - right.numbers[index];
    }
  }
  if (left.prereleaseRank !== right.prereleaseRank) {
    return left.prereleaseRank - right.prereleaseRank;
  }
  return left.prereleaseNumber - right.prereleaseNumber;
}

function encodePath(path) {
  return path.split("/").map(encodeURIComponent).join("/");
}

function markdown(value) {
  return String(value ?? "missing")
    .replaceAll("|", "\\|")
    .replaceAll("\r", " ")
    .replaceAll("\n", " ");
}

function delay(milliseconds) {
  return new Promise((accept) => setTimeout(accept, milliseconds));
}

const manifestPath = resolve(argument("--manifest", "upstream/aseprite-compatibility.json"));
const outputPath = resolve(argument("--output", "aseprite-upstream-report.md"));
const manifest = JSON.parse(await readFile(manifestPath, "utf8"));

ensure(manifest.schema_version === 1, "schema_version must be 1");
ensure(/^\d{4}-\d{2}-\d{2}$/.test(manifest.recorded_at), "invalid recorded_at date");
ensure(manifest.upstream?.repository === "aseprite/aseprite", "unexpected upstream repository");
ensure(manifest.upstream?.default_branch === "main", "unexpected default branch");
ensure(isGitSha(manifest.implementation_origin_commit), "invalid implementation origin commit");
ensure(manifest.compatibility?.series === "1.3", "unexpected compatibility series");
ensure(
  manifest.compatibility?.newer_release_policy === "blocked_until_review",
  "unexpected newer release policy",
);
ensure(
  manifest.compatibility?.portable_release_policy === "additional_pinned_skia_gate",
  "unexpected portable release policy",
);
ensure(
  manifest.baseline_release?.tag === manifest.compatibility?.reviewed_through,
  "baseline tag and reviewed_through must match",
);
ensure(isGitSha(manifest.baseline_release?.commit), "invalid baseline commit");
ensure(manifest.baseline_release?.prerelease === false, "baseline must be stable");
ensure(isDigest(manifest.baseline_release?.source_asset_digest), "invalid source digest");
const expectedSourceAssetName = `Aseprite-${manifest.baseline_release?.tag}-Source.zip`;
ensure(
  manifest.baseline_release?.source_asset_name === expectedSourceAssetName,
  "invalid source asset name",
);
ensure(
  manifest.baseline_release?.source_asset_url ===
    `https://github.com/aseprite/aseprite/releases/download/${manifest.baseline_release?.tag}/${manifest.baseline_release?.source_asset_name}`,
  "invalid source asset URL",
);
ensure(isPositiveInteger(manifest.baseline_release?.release_id), "invalid release ID");
ensure(isPositiveInteger(manifest.baseline_release?.asset_id), "invalid asset ID");
ensure(isPositiveInteger(manifest.baseline_release?.source_asset_size), "invalid asset size");

const reviewedVersion = parseVersion(manifest.compatibility.reviewed_through);
ensure(reviewedVersion?.stable, "reviewed_through must be a stable Aseprite 1.3 tag");

for (const path of ["INSTALL.md", "build.sh"]) {
  const tracked = manifest.tracked_files?.[path];
  ensure(tracked, `missing ${path} contract`);
  ensure(isGitSha(tracked.baseline_blob_sha), `invalid ${path} baseline blob`);
  ensure(isGitSha(tracked.observed_main_blob_sha), `invalid ${path} main blob`);
  ensure(isGitSha(tracked.last_reviewed_change_commit), `invalid ${path} path commit`);
  ensure(
    tracked.immutable_url ===
      `https://github.com/aseprite/aseprite/blob/${manifest.baseline_release.commit}/${path}`,
    `invalid ${path} immutable URL`,
  );
  ensure(
    tracked.current_url === `https://github.com/aseprite/aseprite/blob/main/${path}`,
    `invalid ${path} current URL`,
  );
}

const buildContract = manifest.tracked_files["build.sh"];
ensure(buildContract.scope === "macos", "unexpected build.sh scope");
ensure(
  Array.isArray(buildContract.arguments) &&
    buildContract.arguments.length === 2 &&
    buildContract.arguments[0] === "--auto" &&
    buildContract.arguments[1] === "--norun",
  "unexpected build.sh arguments",
);
ensure(
  buildContract.documented_output === "build/bin/Aseprite.app",
  "unexpected documented build output",
);
ensure(
  buildContract.installer_output_policy ===
    "validated_aseprite_app_bundle_under_build_directory",
  "unexpected installer output policy",
);

function validateCheckerLogic() {
  const stable = parseVersion("v1.3.18.1");
  const beta = parseVersion("v1.3.18-beta2");
  if (
    !stable?.stable ||
    beta?.prereleaseRank !== 1 ||
    compareVersions(stable, beta) <= 0 ||
    parseVersion("v1.3.18-preview1") !== null ||
    releaseSeries("v2.0.0")?.join(".") !== "2.0"
  ) {
    throw new Error("Aseprite version parser self-test failed.");
  }
  const mismatched = {
    tag_name: "v1.3.15.4",
    assets: [{ name: "Aseprite-v1.3.15.5-Source.zip" }],
  };
  if (
    exactSourceAssets(mismatched).length !== 0 ||
    sourceAssets(mismatched).length !== 1 ||
    sourceTag(sourceAssets(mismatched)[0]) !== "v1.3.15.5"
  ) {
    throw new Error("Aseprite source-asset parser self-test failed.");
  }
}

validateCheckerLogic();

if (validateOnly) {
  console.log(`Compatibility manifest is valid: ${manifestPath}`);
  process.exit(0);
}

const apiRoot = process.env.UPSTREAM_GITHUB_API_URL || "https://api.github.com";
const upstreamToken = process.env.UPSTREAM_GITHUB_TOKEN?.trim();
const headers = {
  Accept: "application/vnd.github+json",
  "User-Agent": "aseprite-installer-upstream-watch",
  "X-GitHub-Api-Version": API_VERSION,
};
if (upstreamToken) headers.Authorization = `Bearer ${upstreamToken}`;

async function apiJson(path, { allowNotFound = false } = {}) {
  const url = `${apiRoot}${path}`;
  for (let attempt = 0; attempt < 3; attempt += 1) {
    let response;
    try {
      response = await fetch(url, {
        headers,
        signal: AbortSignal.timeout(20_000),
      });
    } catch (error) {
      if (attempt < 2) {
        await delay(500 * 2 ** attempt);
        continue;
      }
      throw new Error(`GitHub API request failed for ${path}: ${error.message}`);
    }
    if (response.status === 404 && allowNotFound) return null;
    const retryable = response.status === 403 || response.status === 429 || response.status >= 500;
    if (retryable && attempt < 2) {
      await delay(500 * 2 ** attempt);
      continue;
    }
    if (!response.ok) {
      const remaining = response.headers.get("x-ratelimit-remaining");
      throw new Error(
        `GitHub API ${response.status} for ${path} (rate limit remaining: ${remaining ?? "unknown"})`,
      );
    }
    try {
      return await response.json();
    } catch (error) {
      throw new Error(`GitHub API returned invalid JSON for ${path}: ${error.message}`);
    }
  }
  throw new Error(`GitHub API retries exhausted for ${path}`);
}

async function allReleases(repository) {
  const releases = [];
  for (let page = 1; page <= 20; page += 1) {
    const batch = await apiJson(
      `/repos/${repository}/releases?per_page=${pageSize}&page=${page}`,
    );
    if (!Array.isArray(batch)) throw new Error("GitHub releases response is not an array");
    releases.push(...batch);
    if (batch.length < pageSize) return releases;
  }
  throw new Error("GitHub release pagination exceeded the safety limit");
}

async function content(repository, path, ref) {
  const value = await apiJson(
    `/repos/${repository}/contents/${encodePath(path)}?ref=${encodeURIComponent(ref)}`,
    { allowNotFound: true },
  );
  return value && typeof value.sha === "string" ? value.sha : null;
}

async function latestPathCommit(repository, path, branch) {
  const commits = await apiJson(
    `/repos/${repository}/commits?path=${encodeURIComponent(path)}&sha=${encodeURIComponent(branch)}&per_page=1`,
  );
  return Array.isArray(commits) && typeof commits[0]?.sha === "string" ? commits[0].sha : null;
}

async function resolveTagCommit(repository, tag) {
  const reference = await apiJson(
    `/repos/${repository}/git/ref/tags/${encodeURIComponent(tag)}`,
    { allowNotFound: true },
  );
  if (!reference?.object) return null;
  let object = reference.object;
  const seen = new Set();
  for (let depth = 0; depth < 8; depth += 1) {
    if (object.type === "commit" && isGitSha(object.sha)) return object.sha;
    if (object.type !== "tag" || !isGitSha(object.sha) || seen.has(object.sha)) return null;
    seen.add(object.sha);
    const tagObject = await apiJson(`/repos/${repository}/git/tags/${object.sha}`, {
      allowNotFound: true,
    });
    if (!tagObject?.object) return null;
    object = tagObject.object;
  }
  return null;
}

function exactSourceAssets(release) {
  if (!release) return [];
  const expectedName = `Aseprite-${release.tag_name}-Source.zip`;
  return Array.isArray(release.assets)
    ? release.assets.filter((asset) => asset.name === expectedName)
    : [];
}

function sourceAssets(release) {
  return Array.isArray(release?.assets)
    ? release.assets.filter(
        (asset) =>
          typeof asset.name === "string" &&
          /^Aseprite-v[^/]+-Source\.zip$/.test(asset.name),
      )
    : [];
}

function sourceTag(asset) {
  return asset?.name?.match(/^Aseprite-(v[^/]+)-Source\.zip$/)?.[1] ?? null;
}

function releaseSeries(tag) {
  const match = typeof tag === "string" ? tag.match(/^v(\d+)\.(\d+)(?:\.|-|$)/) : null;
  if (!match) return null;
  const series = [Number(match[1]), Number(match[2])];
  return series.every(Number.isSafeInteger) ? series : null;
}

const repository = manifest.upstream.repository;
const branch = manifest.upstream.default_branch;
const projectRepository = process.env.GITHUB_REPOSITORY || "fmhun/aseprite-installer";
const baseline = manifest.baseline_release;
const releases = await allReleases(repository);
const baselineRelease = await apiJson(
  `/repos/${repository}/releases/tags/${encodeURIComponent(baseline.tag)}`,
  { allowNotFound: true },
);
const baselineAssets = exactSourceAssets(baselineRelease);
const baselineAsset = baselineAssets.length === 1 ? baselineAssets[0] : null;

const [
  baselineTagCommit,
  baselineInstallBlob,
  baselineBuildBlob,
  mainInstallBlob,
  mainBuildBlob,
  mainInstallCommit,
  mainBuildCommit,
] = await Promise.all([
  resolveTagCommit(repository, baseline.tag),
  content(repository, "INSTALL.md", baseline.tag),
  content(repository, "build.sh", baseline.tag),
  content(repository, "INSTALL.md", branch),
  content(repository, "build.sh", branch),
  latestPathCommit(repository, "INSTALL.md", branch),
  latestPathCommit(repository, "build.sh", branch),
]);

const knownSeriesReleases = releases.filter(
  (release) => !release.draft && typeof release.tag_name === "string" && release.tag_name.startsWith("v1.3"),
);
const unknownSeriesTags = knownSeriesReleases
  .filter((release) => !parseVersion(release.tag_name))
  .map((release) => release.tag_name)
  .sort();
const newerReleases = knownSeriesReleases
  .map((release) => ({ release, version: parseVersion(release.tag_name) }))
  .filter(({ version }) => version && compareVersions(version, reviewedVersion) > 0)
  .sort((left, right) => compareVersions(right.version, left.version));
const newerSeriesReleases = releases
  .filter((release) => !release.draft)
  .map((release) => ({ release, series: releaseSeries(release.tag_name) }))
  .filter(
    ({ series }) =>
      series && (series[0] > 1 || (series[0] === 1 && series[1] > 3)),
  )
  .sort(
    (left, right) =>
      right.series[0] - left.series[0] || right.series[1] - left.series[1],
  );
const candidate = newerSeriesReleases[0]?.release ?? newerReleases[0]?.release ?? null;

let candidateDetails = null;
if (candidate) {
  const candidateAssets = sourceAssets(candidate);
  const candidateAsset = candidateAssets.length === 1 ? candidateAssets[0] : null;
  const candidateSourceTag = sourceTag(candidateAsset);
  const [commit, installBlob, buildBlob] = await Promise.all([
    resolveTagCommit(repository, candidate.tag_name),
    content(repository, "INSTALL.md", candidate.tag_name),
    content(repository, "build.sh", candidate.tag_name),
  ]);
  candidateDetails = {
    tag: candidate.tag_name,
    prerelease: Boolean(candidate.prerelease),
    htmlUrl:
      candidate.html_url ??
      `https://github.com/${repository}/releases/tag/${encodeURIComponent(candidate.tag_name)}`,
    commit,
    assetCount: candidateAssets.length,
    assetId: candidateAsset?.id ?? null,
    assetName: candidateAsset?.name ?? null,
    sourceTag: candidateSourceTag,
    sourceTagMatchesRelease: candidateSourceTag === candidate.tag_name,
    assetDigest: candidateAsset?.digest ?? null,
    assetSize: candidateAsset?.size ?? null,
    installBlob,
    buildBlob,
  };
}

const checks = [];
function check(label, recorded, observed, matches = recorded === observed) {
  checks.push({ label, recorded, observed, matches });
}

check("Baseline release ID", baseline.release_id, baselineRelease?.id ?? null);
check("Baseline tag commit", baseline.commit, baselineTagCommit);
check("Baseline prerelease flag", baseline.prerelease, baselineRelease?.prerelease ?? null);
check("Baseline source asset count", 1, baselineAssets.length);
check("Baseline source asset ID", baseline.asset_id, baselineAsset?.id ?? null);
check("Baseline source asset name", baseline.source_asset_name, baselineAsset?.name ?? null);
check(
  "Baseline source asset URL",
  baseline.source_asset_url,
  baselineAsset?.browser_download_url ?? null,
);
check("Baseline source asset digest", baseline.source_asset_digest, baselineAsset?.digest ?? null);
check("Baseline source asset size", baseline.source_asset_size, baselineAsset?.size ?? null);
check(
  "Baseline INSTALL.md blob",
  manifest.tracked_files["INSTALL.md"].baseline_blob_sha,
  baselineInstallBlob,
);
check(
  "Baseline build.sh blob",
  manifest.tracked_files["build.sh"].baseline_blob_sha,
  baselineBuildBlob,
);
check(
  "Observed main INSTALL.md blob",
  manifest.tracked_files["INSTALL.md"].observed_main_blob_sha,
  mainInstallBlob,
);
check(
  "Observed main INSTALL.md path commit",
  manifest.tracked_files["INSTALL.md"].last_reviewed_change_commit,
  mainInstallCommit,
);
check(
  "Observed main build.sh blob",
  manifest.tracked_files["build.sh"].observed_main_blob_sha,
  mainBuildBlob,
);
check(
  "Observed main build.sh path commit",
  manifest.tracked_files["build.sh"].last_reviewed_change_commit,
  mainBuildCommit,
);
check(
  "Releases newer than reviewed_through",
  "none",
  newerReleases.length ? newerReleases.map(({ release }) => release.tag_name).join(", ") : "none",
  newerReleases.length === 0,
);
check(
  "Releases in a newer Aseprite series",
  "none",
  newerSeriesReleases.length
    ? newerSeriesReleases.map(({ release }) => release.tag_name).join(", ")
    : "none",
  newerSeriesReleases.length === 0,
);
check(
  "Unknown Aseprite 1.3 tag formats",
  "none",
  unknownSeriesTags.length ? unknownSeriesTags.join(", ") : "none",
  unknownSeriesTags.length === 0,
);

const drift = checks.some((item) => !item.matches);
const lines = [
  ISSUE_MARKER,
  "# Aseprite upstream compatibility review",
  "",
  drift
    ? "Upstream state differs from `upstream/aseprite-compatibility.json`. A maintainer review is required."
    : "No upstream drift was detected.",
  "",
  `Reviewed-through release: \`${markdown(manifest.compatibility.reviewed_through)}\``,
  "",
  "| Check | Recorded | Observed | State |",
  "| --- | --- | --- | --- |",
  ...checks.map(
    (item) =>
      `| ${markdown(item.label)} | \`${markdown(item.recorded)}\` | \`${markdown(item.observed)}\` | ${item.matches ? "OK" : "REVIEW"} |`,
  ),
];

if (candidateDetails) {
  lines.push(
    "",
    "## Highest unreviewed release",
    "",
    `- Tag: [${markdown(candidateDetails.tag)}](${candidateDetails.htmlUrl})`,
    `- Prerelease: \`${candidateDetails.prerelease}\``,
    `- Resolved commit: \`${markdown(candidateDetails.commit)}\``,
    `- Source assets found: \`${candidateDetails.assetCount}\``,
    `- Source asset ID: \`${markdown(candidateDetails.assetId)}\``,
    `- Source tag: \`${markdown(candidateDetails.sourceTag)}\``,
    `- Source tag matches release tag: \`${candidateDetails.sourceTagMatchesRelease}\``,
    `- Source digest: \`${markdown(candidateDetails.assetDigest)}\``,
    `- Source size: \`${markdown(candidateDetails.assetSize)}\``,
    `- INSTALL.md blob: \`${markdown(candidateDetails.installBlob)}\``,
    `- build.sh blob: \`${markdown(candidateDetails.buildBlob)}\``,
    `- [Compare with the reviewed release](https://github.com/${repository}/compare/${encodeURIComponent(baseline.tag)}...${encodeURIComponent(candidateDetails.tag)})`,
  );
}

if (drift) {
  lines.push(
    "",
    "## Required review",
    "",
    "- Inspect upstream diffs for `INSTALL.md` and `build.sh`.",
    "- Review prerequisites, script arguments, archive layout, Skia selection, and output path.",
    "- Verify the candidate tag, source asset identity, digest, and size.",
    "- Apply compatibility fixes and run the documented frontend and Rust checks.",
    "- Complete local macOS arm64, macOS x64, Linux x64, and Windows x64 builds before raising `reviewed_through`.",
    "- Update the manifest in the same pull request; do not upload Aseprite build artifacts.",
    "",
    `See [the upstream compatibility procedure](https://github.com/${projectRepository}/blob/main/docs/UPSTREAM_COMPATIBILITY.md) for the full checklist.`,
  );
}

const report = `${lines.join("\n")}\n`;
await mkdir(dirname(outputPath), { recursive: true });
await writeFile(outputPath, report, "utf8");

if (process.env.GITHUB_OUTPUT) {
  await appendFile(process.env.GITHUB_OUTPUT, `drift=${drift}\n`, "utf8");
}

console.log(drift ? "Aseprite upstream drift detected." : "Aseprite upstream is unchanged.");
console.log(`Report written to ${outputPath}`);
