#!/usr/bin/env bash

set -euo pipefail

if [[ "$#" != 6 ]]; then
  echo "Usage: $0 INPUT_DIRECTORY OUTPUT_DIRECTORY PUBLIC_KEY SOURCES_FILE PREFERENCES_FILE EXPECTED_FINGERPRINT" >&2
  exit 2
fi

input_directory="$1"
output_directory="$2"
public_key="$3"
sources_file="$4"
preferences_file="$5"
expected_fingerprint="${6//[[:space:]]/}"

for command_name in apt-ftparchive dpkg-deb gpg gzip install sha256sum xz; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "Missing required command: $command_name" >&2
    exit 1
  fi
done

if [[ ! -d "$input_directory" || ! -f "$public_key" || -L "$public_key" || ! -f "$sources_file" || -L "$sources_file" || ! -f "$preferences_file" || -L "$preferences_file" ]]; then
  echo "Input packages, a regular public key, source definition, and preference file are required." >&2
  exit 1
fi
if [[ ! "$expected_fingerprint" =~ ^[A-F0-9]{40}$ ]]; then
  echo "Expected an uppercase 40-character OpenPGP fingerprint." >&2
  exit 1
fi

mkdir -p "$output_directory"
if find "$output_directory" -mindepth 1 -print -quit | grep -q .; then
  echo "Output directory must be empty: $output_directory" >&2
  exit 1
fi

actual_fingerprint="$(
  gpg --batch --with-colons --show-keys "$public_key" \
    | awk -F: '$1 == "fpr" { print $10; exit }'
)"
if [[ "$actual_fingerprint" != "$expected_fingerprint" ]]; then
  echo "Public-key fingerprint mismatch: expected $expected_fingerprint, received $actual_fingerprint." >&2
  exit 1
fi
if ! gpg --batch --with-colons --list-secret-keys "$expected_fingerprint" \
  | awk -F: '$1 == "sec" && $12 ~ /s/ { found=1 } END { exit !found }'; then
  echo "The exact APT signing secret key is not available." >&2
  exit 1
fi

pool_directory="$output_directory/pool/main/a/aseprite-installer"
index_directory="$output_directory/dists/stable/main/binary-amd64"
mkdir -p "$pool_directory" "$index_directory"

mapfile -d '' package_files < <(find "$input_directory" -type f -name '*.deb' -print0 | LC_ALL=C sort -z)
if (( ${#package_files[@]} == 0 )); then
  echo "No deb packages were supplied." >&2
  exit 1
fi

declare -A seen_versions=()
for package_file in "${package_files[@]}"; do
  if [[ -L "$package_file" ]]; then
    echo "Refusing symlinked package input: $package_file" >&2
    exit 1
  fi

  package_name="$(dpkg-deb --field "$package_file" Package)"
  package_version="$(dpkg-deb --field "$package_file" Version)"
  package_architecture="$(dpkg-deb --field "$package_file" Architecture)"
  release_tag="$(basename "$(dirname "$package_file")")"

  if [[ "$package_name" != "aseprite-installer" ]]; then
    echo "Unexpected deb package name: $package_name" >&2
    exit 1
  fi
  if [[ "$package_architecture" != "amd64" ]]; then
    echo "Unexpected deb architecture: $package_architecture" >&2
    exit 1
  fi
  if [[ ! "$release_tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Package input must be nested under its stable release tag: $package_file" >&2
    exit 1
  fi
  if [[ "$package_version" != "${release_tag#v}" ]]; then
    echo "Package version $package_version does not match release tag $release_tag." >&2
    exit 1
  fi
  if [[ -n "${seen_versions[$package_version]:-}" ]]; then
    echo "Duplicate package version: $package_version" >&2
    exit 1
  fi
  seen_versions[$package_version]=1

  canonical_name="aseprite-installer_${package_version}_amd64.deb"
  install -m 0644 "$package_file" "$pool_directory/$canonical_name"
done

(
  cd "$output_directory"
  apt-ftparchive packages pool > "dists/stable/main/binary-amd64/Packages"
)
gzip -9n -c "$index_directory/Packages" > "$index_directory/Packages.gz"
xz -9e --threads=1 -c "$index_directory/Packages" > "$index_directory/Packages.xz"

by_hash_directory="$index_directory/by-hash/SHA256"
mkdir -p "$by_hash_directory"
for index_name in Packages Packages.gz Packages.xz; do
  index_digest="$(sha256sum "$index_directory/$index_name" | awk '{print $1}')"
  install -m 0644 "$index_directory/$index_name" "$by_hash_directory/$index_digest"
done

release_directory="$output_directory/dists/stable"
valid_until="${APT_REPOSITORY_VALID_UNTIL:-$(date --utc --date='+90 days' --rfc-email)}"
release_body="$release_directory/Release.body"
(
  cd "$output_directory"
  apt-ftparchive \
    -o APT::FTPArchive::Release::Origin="Aseprite Installer" \
    -o APT::FTPArchive::Release::Label="Aseprite Installer" \
    -o APT::FTPArchive::Release::Suite="stable" \
    -o APT::FTPArchive::Release::Codename="stable" \
    -o APT::FTPArchive::Release::Architectures="amd64" \
    -o APT::FTPArchive::Release::Components="main" \
    -o APT::FTPArchive::Release::Acquire-By-Hash="yes" \
    release dists/stable > "dists/stable/Release.body"
)
if ! grep -q '^Date: ' "$release_body"; then
  echo "apt-ftparchive did not generate the required Date field." >&2
  exit 1
fi
awk -v valid_until="$valid_until" '
  /^Date: / {
    print
    print "Valid-Until: " valid_until
    next
  }
  { print }
' "$release_body" > "$release_directory/Release"
rm -- "$release_body"

gpg --batch --yes --pinentry-mode loopback \
  --local-user "$expected_fingerprint" \
  --digest-algo SHA512 \
  --clearsign \
  --output "$release_directory/InRelease" \
  "$release_directory/Release"
gpg --batch --yes --pinentry-mode loopback \
  --local-user "$expected_fingerprint" \
  --digest-algo SHA512 \
  --armor --detach-sign \
  --output "$release_directory/Release.gpg" \
  "$release_directory/Release"

install -m 0644 "$public_key" "$output_directory/aseprite-installer-archive-keyring.asc"
install -m 0644 "$sources_file" "$output_directory/aseprite-installer.sources"
install -m 0644 "$preferences_file" "$output_directory/aseprite-installer.pref"

cat > "$output_directory/README.txt" <<EOF
Aseprite Installer signed APT repository

Archive key fingerprint: $expected_fingerprint
Repository: https://fmhun.github.io/aseprite-installer/apt/
Bootstrap: https://fmhun.github.io/aseprite-installer/install-apt.sh

This repository distributes Aseprite Installer only. It does not distribute Aseprite.
EOF

if grep -R -I -l -- 'BEGIN PGP PRIVATE KEY' "$output_directory" | grep -q .; then
  echo "Refusing to publish private-key material." >&2
  exit 1
fi

printf 'Built signed APT repository with %d package version(s).\n' "${#package_files[@]}"
