#!/usr/bin/env bash

set -euo pipefail

if [[ "$#" != 5 ]]; then
  echo "Usage: $0 REPOSITORY_DIRECTORY PUBLIC_KEY SOURCES_FILE PREFERENCES_FILE EXPECTED_FINGERPRINT" >&2
  exit 2
fi

repository_directory="$1"
public_key="$2"
sources_file="$3"
preferences_file="$4"
expected_fingerprint="${5//[[:space:]]/}"

for command_name in apt-cache apt-ftparchive apt-get dpkg-deb gpg gpgv gzip sha256sum xz; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "Missing required command: $command_name" >&2
    exit 1
  fi
done

release_directory="$repository_directory/dists/stable"
index_directory="$release_directory/main/binary-amd64"
required_files=(
  "$repository_directory/aseprite-installer-archive-keyring.asc"
  "$repository_directory/aseprite-installer.sources"
  "$repository_directory/aseprite-installer.pref"
  "$release_directory/InRelease"
  "$release_directory/Release"
  "$release_directory/Release.gpg"
  "$index_directory/Packages"
  "$index_directory/Packages.gz"
  "$index_directory/Packages.xz"
)

for required_file in "${required_files[@]}"; do
  if [[ ! -f "$required_file" || -L "$required_file" ]]; then
    echo "Missing regular repository file: $required_file" >&2
    exit 1
  fi
done
if find "$repository_directory" -type l -print -quit | grep -q .; then
  echo "Repository output must not contain symlinks." >&2
  exit 1
fi
if grep -R -I -l -- 'BEGIN PGP PRIVATE KEY' "$repository_directory" | grep -q .; then
  echo "Repository output contains private-key material." >&2
  exit 1
fi
if ! cmp -s "$public_key" "$repository_directory/aseprite-installer-archive-keyring.asc"; then
  echo "Published archive key does not match the committed public key." >&2
  exit 1
fi
if [[ ! -f "$sources_file" || -L "$sources_file" ]] \
  || ! cmp -s "$sources_file" "$repository_directory/aseprite-installer.sources"; then
  echo "Published source definition does not match the committed source definition." >&2
  exit 1
fi
if [[ ! -f "$preferences_file" || -L "$preferences_file" ]] \
  || ! cmp -s "$preferences_file" "$repository_directory/aseprite-installer.pref"; then
  echo "Published APT preference does not match the committed origin policy." >&2
  exit 1
fi

actual_fingerprint="$(
  gpg --batch --with-colons --show-keys "$public_key" \
    | awk -F: '$1 == "fpr" { print $10; exit }'
)"
if [[ "$actual_fingerprint" != "$expected_fingerprint" ]]; then
  echo "Archive-key fingerprint mismatch." >&2
  exit 1
fi

temporary_directory="$(mktemp -d)"
cleanup() {
  if [[ -d "$temporary_directory" ]]; then
    rm -r -- "$temporary_directory"
  fi
}
trap cleanup EXIT

gpg --batch --yes --dearmor \
  --output "$temporary_directory/archive-keyring.gpg" \
  "$public_key"
gpgv --keyring "$temporary_directory/archive-keyring.gpg" \
  --output "$temporary_directory/InRelease.payload" \
  "$release_directory/InRelease"
gpgv --keyring "$temporary_directory/archive-keyring.gpg" \
  "$release_directory/Release.gpg" \
  "$release_directory/Release"
cmp "$temporary_directory/InRelease.payload" "$release_directory/Release"

gzip --decompress --stdout "$index_directory/Packages.gz" \
  | cmp - "$index_directory/Packages"
xz --decompress --stdout "$index_directory/Packages.xz" \
  | cmp - "$index_directory/Packages"

for index_name in Packages Packages.gz Packages.xz; do
  index_digest="$(sha256sum "$index_directory/$index_name" | awk '{print $1}')"
  cmp "$index_directory/$index_name" "$index_directory/by-hash/SHA256/$index_digest"
done

for expected_field in \
  'Origin: Aseprite Installer' \
  'Label: Aseprite Installer' \
  'Suite: stable' \
  'Codename: stable' \
  'Architectures: amd64' \
  'Components: main' \
  'Acquire-By-Hash: yes'; do
  grep -Fx "$expected_field" "$release_directory/Release" >/dev/null
done

valid_until="$(sed -n 's/^Valid-Until: //p' "$release_directory/Release")"
if [[ -z "$valid_until" ]] || (( $(date --utc --date="$valid_until" +%s) <= $(date --utc +%s) )); then
  echo "Repository metadata is expired or missing Valid-Until." >&2
  exit 1
fi

(
  cd "$release_directory"
  awk '
    $1 == "SHA256:" { in_sha256=1; next }
    in_sha256 && /^ / { print $1 "  " $3; next }
    in_sha256 { exit }
  ' Release | sha256sum --check --strict -
)

regenerated_packages="$temporary_directory/Packages"
(
  cd "$repository_directory"
  apt-ftparchive packages pool > "$regenerated_packages"
)
cmp "$regenerated_packages" "$index_directory/Packages"

mapfile -d '' pool_packages < <(find "$repository_directory/pool" -type f -name '*.deb' -print0 | LC_ALL=C sort -z)
if (( ${#pool_packages[@]} == 0 )); then
  echo "Repository pool contains no deb package." >&2
  exit 1
fi
for pool_package in "${pool_packages[@]}"; do
  [[ "$(dpkg-deb --field "$pool_package" Package)" == "aseprite-installer" ]]
  [[ "$(dpkg-deb --field "$pool_package" Architecture)" == "amd64" ]]
done

apt_root="$temporary_directory/apt"
mkdir -p \
  "$apt_root/state/lists/partial" \
  "$apt_root/cache/archives/partial" \
  "$apt_root/download"
cat > "$apt_root/sources.list" <<EOF
deb [arch=amd64 signed-by=$temporary_directory/archive-keyring.gpg] file:$repository_directory stable main
EOF

apt_options=(
  -o "Dir::Etc::sourcelist=$apt_root/sources.list"
  -o "Dir::Etc::sourceparts=-"
  -o "Dir::State=$apt_root/state"
  -o "Dir::Cache=$apt_root/cache"
  -o "Debug::NoLocking=1"
  -o "APT::Get::List-Cleanup=0"
)
apt-get "${apt_options[@]}" update >/dev/null
apt-cache "${apt_options[@]}" show aseprite-installer \
  | grep -Fx 'Package: aseprite-installer' >/dev/null
(
  cd "$apt_root/download"
  apt-get "${apt_options[@]}" download aseprite-installer >/dev/null
)

mapfile -d '' downloaded_packages < <(find "$apt_root/download" -type f -name '*.deb' -print0)
if (( ${#downloaded_packages[@]} != 1 )); then
  echo "APT did not download exactly one candidate package." >&2
  exit 1
fi
downloaded_digest="$(sha256sum "${downloaded_packages[0]}" | awk '{print $1}')"
if ! printf '%s\n' "${pool_packages[@]}" \
  | xargs sha256sum \
  | awk '{print $1}' \
  | grep -Fx "$downloaded_digest" >/dev/null; then
  echo "APT candidate does not match a package in the repository pool." >&2
  exit 1
fi

echo "APT repository signatures, metadata, package pool, and client resolution are valid."
