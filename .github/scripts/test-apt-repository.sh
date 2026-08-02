#!/usr/bin/env bash

set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"

for command_name in apt-ftparchive dpkg-deb gpg shellcheck; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "Missing test dependency: $command_name" >&2
    exit 1
  fi
done

bash -n \
  "$repository_root/.github/scripts/build-apt-repository.sh" \
  "$repository_root/.github/scripts/download-apt-release-packages.sh" \
  "$repository_root/.github/scripts/verify-apt-repository.sh"
dash -n "$repository_root/site/public/install-apt.sh"
shellcheck \
  "$repository_root/.github/scripts/build-apt-repository.sh" \
  "$repository_root/.github/scripts/download-apt-release-packages.sh" \
  "$repository_root/.github/scripts/test-apt-repository.sh" \
  "$repository_root/.github/scripts/verify-apt-repository.sh" \
  "$repository_root/site/public/install-apt.sh"

if grep -Eq 'apt-key|trusted=yes|allow-unauthenticated|--allow-unauthenticated' \
  "$repository_root/site/public/install-apt.sh"; then
  echo "Bootstrap script contains an insecure APT bypass." >&2
  exit 1
fi

temporary_directory="$(mktemp -d)"
cleanup() {
  if [[ -d "$temporary_directory" ]]; then
    rm -r -- "$temporary_directory"
  fi
}
trap cleanup EXIT

export GNUPGHOME="$temporary_directory/gnupg"
mkdir -m 0700 "$GNUPGHOME"
gpg --batch --pinentry-mode loopback --passphrase '' \
  --quick-generate-key 'Aseprite Installer APT Test <test@example.invalid>' ed25519 sign 1d \
  >/dev/null 2>&1
test_fingerprint="$(
  gpg --batch --with-colons --list-secret-keys \
    | awk -F: '$1 == "fpr" { print $10; exit }'
)"
test_public_key="$temporary_directory/test-public.asc"
gpg --batch --armor --export "$test_fingerprint" > "$test_public_key"

package_root="$temporary_directory/package-root"
mkdir -p "$package_root/DEBIAN" "$package_root/usr/share/doc/aseprite-installer"
cat > "$package_root/DEBIAN/control" <<'CONTROL'
Package: aseprite-installer
Version: 0.0.1
Section: graphics
Priority: optional
Architecture: amd64
Maintainer: Aseprite Installer Tests <test@example.invalid>
Description: inert APT repository test fixture
CONTROL
printf '%s\n' 'This package is an inert repository fixture.' \
  > "$package_root/usr/share/doc/aseprite-installer/README"

input_directory="$temporary_directory/input/v0.0.1"
mkdir -p "$input_directory"
dpkg-deb --root-owner-group --build \
  "$package_root" \
  "$input_directory/Aseprite-Installer-Linux-x86_64.deb" \
  >/dev/null

release_fixture="$temporary_directory/release-fixture/v0.0.1"
mkdir -p "$release_fixture"
cp "$input_directory/Aseprite-Installer-Linux-x86_64.deb" "$release_fixture/"
(
  cd "$release_fixture"
  sha256sum Aseprite-Installer-Linux-x86_64.deb > SHA256SUMS
)

mock_bin="$temporary_directory/mock-bin"
mkdir -p "$mock_bin"
cat > "$mock_bin/gh" <<'MOCK_GH'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}:${2:-}" in
  release:list)
    printf '%s\n' v0.0.1
    ;;
  release:view)
    if [[ "${APT_TEST_FAIL_RELEASE_VIEW:-0}" == 1 ]]; then
      exit 17
    fi
    printf '%s\n' Aseprite-Installer-Linux-x86_64.deb SHA256SUMS
    ;;
  release:download)
    destination=''
    while (( $# > 0 )); do
      if [[ "$1" == --dir ]]; then
        destination="$2"
        shift 2
        continue
      fi
      shift
    done
    [[ -n "$destination" ]]
    cp "$APT_TEST_RELEASE_DIRECTORY/Aseprite-Installer-Linux-x86_64.deb" "$destination/"
    cp "$APT_TEST_RELEASE_DIRECTORY/SHA256SUMS" "$destination/"
    ;;
  attestation:verify)
    [[ -f "${3:-}" ]]
    [[ " $* " == *" --source-ref refs/tags/v0.0.1 "* ]]
    [[ " $* " == *" --signer-workflow test/example/.github/workflows/release.yml "* ]]
    [[ " $* " == *" --deny-self-hosted-runners "* ]]
    ;;
  *)
    echo "Unexpected mocked gh command: $*" >&2
    exit 2
    ;;
esac
MOCK_GH
chmod 0755 "$mock_bin/gh"

downloaded_input="$temporary_directory/downloaded-input"
PATH="$mock_bin:$PATH" \
  GITHUB_REPOSITORY=test/example \
  APT_TEST_RELEASE_DIRECTORY="$release_fixture" \
  "$repository_root/.github/scripts/download-apt-release-packages.sh" \
    "$downloaded_input" 1
cmp \
  "$release_fixture/Aseprite-Installer-Linux-x86_64.deb" \
  "$downloaded_input/v0.0.1/Aseprite-Installer-Linux-x86_64.deb"

if PATH="$mock_bin:$PATH" \
  GITHUB_REPOSITORY=test/example \
  APT_TEST_RELEASE_DIRECTORY="$release_fixture" \
  APT_TEST_FAIL_RELEASE_VIEW=1 \
  "$repository_root/.github/scripts/download-apt-release-packages.sh" \
    "$temporary_directory/failed-download" 1 >/dev/null 2>&1; then
  echo "A GitHub API failure was not propagated by the release downloader." >&2
  exit 1
fi

output_directory="$temporary_directory/repository"
APT_REPOSITORY_VALID_UNTIL='Thu, 01 Jan 2099 00:00:00 +0000' \
  "$repository_root/.github/scripts/build-apt-repository.sh" \
    "$downloaded_input" \
    "$output_directory" \
    "$test_public_key" \
    "$repository_root/site/public/apt/aseprite-installer.sources" \
    "$repository_root/site/public/apt/aseprite-installer.pref" \
    "$test_fingerprint"
"$repository_root/.github/scripts/verify-apt-repository.sh" \
  "$output_directory" \
  "$test_public_key" \
  "$repository_root/site/public/apt/aseprite-installer.sources" \
  "$repository_root/site/public/apt/aseprite-installer.pref" \
  "$test_fingerprint"
(
  cd "$temporary_directory"
  "$repository_root/.github/scripts/verify-apt-repository.sh" \
    repository \
    "$test_public_key" \
    "$repository_root/site/public/apt/aseprite-installer.sources" \
    "$repository_root/site/public/apt/aseprite-installer.pref" \
    "$test_fingerprint"
)

tampered_repository="$temporary_directory/tampered-repository"
cp -a "$output_directory" "$tampered_repository"
tampered_package="$(find "$tampered_repository/pool" -type f -name '*.deb' -print -quit)"
printf 'tampered\n' >> "$tampered_package"
if "$repository_root/.github/scripts/verify-apt-repository.sh" \
  "$tampered_repository" \
  "$test_public_key" \
  "$repository_root/site/public/apt/aseprite-installer.sources" \
  "$repository_root/site/public/apt/aseprite-installer.pref" \
  "$test_fingerprint" >/dev/null 2>&1; then
  echo "Tampered package unexpectedly passed APT repository verification." >&2
  exit 1
fi

echo "APT repository integration and tamper tests passed."
