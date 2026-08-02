#!/usr/bin/env bash

set -euo pipefail

if [[ "$#" -lt 1 || "$#" -gt 2 ]]; then
  echo "Usage: $0 OUTPUT_DIRECTORY [RELEASE_LIMIT]" >&2
  exit 2
fi

output_directory="$1"
release_limit="${2:-10}"
repository="${GITHUB_REPOSITORY:-fmhun/aseprite-installer}"
asset_name="Aseprite-Installer-Linux-x86_64.deb"

for command_name in gh grep sha256sum; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "Missing required command: $command_name" >&2
    exit 1
  fi
done

if [[ ! "$release_limit" =~ ^[1-9][0-9]*$ ]] || (( release_limit > 25 )); then
  echo "Release limit must be an integer between 1 and 25." >&2
  exit 2
fi

mkdir -p "$output_directory"
if find "$output_directory" -mindepth 1 -print -quit | grep -q .; then
  echo "Output directory must be empty: $output_directory" >&2
  exit 1
fi

temporary_directory="$(mktemp -d)"
cleanup() {
  if [[ -d "$temporary_directory" ]]; then
    rm -r -- "$temporary_directory"
  fi
}
trap cleanup EXIT

if ! release_tags_output="$(
  gh release list \
    --repo "$repository" \
    --exclude-drafts \
    --exclude-pre-releases \
    --limit "$release_limit" \
    --json tagName \
    --jq '.[].tagName'
)"; then
  echo "Could not list releases for $repository." >&2
  exit 1
fi
release_tags=()
while IFS= read -r release_tag; do
  [[ -n "$release_tag" ]] && release_tags+=("$release_tag")
done <<< "$release_tags_output"

downloaded_count=0
for release_tag in "${release_tags[@]}"; do
  if [[ ! "$release_tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    continue
  fi

  if ! release_assets_output="$(
    gh release view "$release_tag" \
      --repo "$repository" \
      --json assets \
      --jq '.assets[].name'
  )"; then
    echo "Could not inspect release $release_tag." >&2
    exit 1
  fi
  release_assets=()
  while IFS= read -r release_asset; do
    [[ -n "$release_asset" ]] && release_assets+=("$release_asset")
  done <<< "$release_assets_output"

  has_package=false
  has_checksums=false
  for release_asset in "${release_assets[@]}"; do
    [[ "$release_asset" == "$asset_name" ]] && has_package=true
    [[ "$release_asset" == "SHA256SUMS" ]] && has_checksums=true
  done

  if [[ "$has_package" == false ]]; then
    continue
  fi
  if [[ "$has_checksums" == false ]]; then
    echo "Release $release_tag contains $asset_name without SHA256SUMS." >&2
    exit 1
  fi

  release_directory="$temporary_directory/$release_tag"
  mkdir -p "$release_directory"
  gh release download "$release_tag" \
    --repo "$repository" \
    --dir "$release_directory" \
    --pattern "$asset_name" \
    --pattern SHA256SUMS

  mapfile -t checksum_lines < <(
    grep -E '^[a-f0-9]{64}  Aseprite-Installer-Linux-x86_64\.deb$' \
      "$release_directory/SHA256SUMS" || true
  )
  if (( ${#checksum_lines[@]} != 1 )); then
    echo "Release $release_tag must contain exactly one checksum for $asset_name." >&2
    exit 1
  fi
  read -r checksum_digest checksum_asset <<< "${checksum_lines[0]}"
  if [[ ! "$checksum_digest" =~ ^[a-f0-9]{64}$ || "$checksum_asset" != "$asset_name" ]]; then
    echo "Release $release_tag contains a malformed checksum for $asset_name." >&2
    exit 1
  fi
  (
    cd "$release_directory"
    printf '%s  %s\n' "$checksum_digest" "$checksum_asset" \
      | sha256sum --check --strict -
  )
  gh attestation verify "$release_directory/$asset_name" \
    --repo "$repository" \
    --source-ref "refs/tags/$release_tag" \
    --signer-workflow "$repository/.github/workflows/release.yml" \
    --deny-self-hosted-runners \
    >/dev/null

  destination_directory="$output_directory/$release_tag"
  mkdir -p "$destination_directory"
  install -m 0644 "$release_directory/$asset_name" "$destination_directory/$asset_name"
  downloaded_count=$((downloaded_count + 1))
done

if (( downloaded_count == 0 )); then
  echo "No stable release with a verified $asset_name asset was found." >&2
  exit 1
fi

printf 'Collected %d verified APT package release(s).\n' "$downloaded_count"
