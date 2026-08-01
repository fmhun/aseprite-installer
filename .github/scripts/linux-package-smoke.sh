#!/usr/bin/env bash

set -euo pipefail

readonly EXPECTED_PACKAGE_NAME="aseprite-installer"
readonly EXPECTED_EXECUTABLE="/usr/bin/aseprite-installer"
readonly DEBIAN_VERSION="12"
readonly FEDORA_VERSION="43"

fail() {
  echo "Linux package verification failed: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "missing command: $1"
}

mode="${1:-}"
shift || true

work_root="$(mktemp -d "${TMPDIR:-/tmp}/aseprite-installer-package-smoke.XXXXXX")"
deb_package_to_remove=""
rpm_package_to_remove=""

cleanup() {
  if [[ -n "$deb_package_to_remove" ]] && command -v dpkg-query >/dev/null 2>&1; then
    local cleanup_deb_status
    cleanup_deb_status="$(dpkg-query --show --showformat='${db:Status-Abbrev}' "$deb_package_to_remove" 2>/dev/null || true)"
    if [[ -n "$cleanup_deb_status" && "$cleanup_deb_status" != un* && "$cleanup_deb_status" != rc* ]]; then
      sudo env DEBIAN_FRONTEND=noninteractive apt-get remove --yes "$deb_package_to_remove" >/dev/null 2>&1 || true
    fi
  fi
  if [[ -n "$rpm_package_to_remove" ]] && command -v rpm >/dev/null 2>&1; then
    if rpm --query "$rpm_package_to_remove" >/dev/null 2>&1; then
      dnf5 --assumeyes remove "$rpm_package_to_remove" >/dev/null 2>&1 || true
    fi
  fi
  if [[ -d "$work_root" ]]; then
    rm -r -- "$work_root"
  fi
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

assert_no_forbidden_dependencies() {
  local package_kind="$1"
  local dependencies="$2"

  if grep -Eiq '(^|[[:space:],|])(aseprite|skia|cmake|ninja(-build)?|build-essential|gcc|g\+\+|clang|make|pkg-config|sudo|curl|wget|git|apt|dnf|yum)([[:space:],()<>:=|]|$)' <<<"$dependencies"; then
    echo "$dependencies" >&2
    fail "$package_kind declares an upstream, build-tool, elevation, network-client, or package-manager dependency"
  fi
}

assert_no_forbidden_payload() {
  local payload_root="$1"

  if find "$payload_root" -xdev \
    \( -type f -o -type l \) \
    \( \
      -iname 'aseprite' -o \
      -iname 'aseprite.exe' -o \
      -iname 'Aseprite.app' -o \
      -iname 'Aseprite-v*-Source.zip' -o \
      -iname 'Skia-*-Release-*.zip' -o \
      -iname 'libskia*' -o \
      -iname 'skia.lib' -o \
      -iname 'skia.dll' -o \
      -iname 'icudtl.dat' \
    \) -print -quit | grep -q .; then
    fail "package contains a forbidden Aseprite or Skia payload"
  fi

  if find "$payload_root" -xdev ! -type d ! -type f ! -type l -print -quit | grep -q .; then
    fail "package contains a device, socket, FIFO, or another unsupported special file"
  fi
  if find "$payload_root" -xdev -type f -perm /6000 -print -quit | grep -q .; then
    fail "package contains a setuid or setgid file"
  fi
  if find "$payload_root" -xdev -type f -perm /0022 -print -quit | grep -q .; then
    fail "package contains a group- or world-writable file"
  fi

  local canonical_root
  canonical_root="$(realpath "$payload_root")"
  while IFS= read -r -d '' link_path; do
    local link_target resolved_target
    link_target="$(readlink "$link_path")"
    if [[ "$link_target" == /* ]]; then
      fail "package contains an absolute symlink: ${link_path#"$payload_root"/} -> $link_target"
    fi
    resolved_target="$(realpath --canonicalize-missing "$(dirname "$link_path")/$link_target")"
    if [[ "$resolved_target" != "$canonical_root" && "$resolved_target" != "$canonical_root/"* ]]; then
      fail "package contains a symlink escaping its payload: ${link_path#"$payload_root"/} -> $link_target"
    fi
  done < <(find "$payload_root" -xdev -type l -print0)
}

assert_desktop_payload() {
  local payload_root="$1"
  local desktop_count=0

  while IFS= read -r -d '' desktop_file; do
    desktop_count=$((desktop_count + 1))
    desktop-file-validate "$desktop_file"
    if ! grep -Eq '^Exec=(/usr/bin/)?aseprite-installer([[:space:]]|$)' "$desktop_file"; then
      fail "desktop launcher does not execute aseprite-installer directly: ${desktop_file#"$payload_root"/}"
    fi
  done < <(find "$payload_root" -xdev -type f -name '*.desktop' -print0)

  if [[ "$desktop_count" -lt 1 ]]; then
    fail "package does not contain a desktop launcher"
  fi
  if ! find "$payload_root" -xdev -type f \
    \( -iname '*.png' -o -iname '*.svg' -o -iname '*.xpm' \) \
    -path '*/icons/*' -print -quit | grep -q .; then
    fail "package does not contain a desktop icon"
  fi
}

assert_application_tree() {
  local payload_root="$1"
  local executable="$payload_root$EXPECTED_EXECUTABLE"

  assert_no_forbidden_payload "$payload_root"
  [[ -x "$executable" ]] || fail "package is missing executable $EXPECTED_EXECUTABLE"
  file "$executable" | grep -Eq 'ELF 64-bit.*x86-64' || fail "application executable is not an x86_64 ELF"
  if readelf --wide --program-headers "$executable" | grep 'GNU_STACK' | grep -q 'RWE'; then
    fail "application executable requests an executable stack"
  fi
  assert_desktop_payload "$payload_root"
}

verify_appimage() {
  local appimage
  appimage="$(realpath "$1")"
  [[ -f "$appimage" && ! -L "$appimage" && -x "$appimage" ]] || fail "AppImage must be one executable regular file"
  file "$appimage" | grep -q 'x86-64' || fail "AppImage runtime is not x86_64"

  local extract_root="$work_root/appimage"
  mkdir "$extract_root"
  (
    cd "$extract_root"
    "$appimage" --appimage-extract >"$work_root/appimage-extract.log"
  )
  [[ -x "$extract_root/squashfs-root/AppRun" ]] || fail "AppImage extraction did not produce an executable AppRun"
  assert_application_tree "$extract_root/squashfs-root"
}

verify_deb() {
  local deb
  deb="$(realpath "$1")"
  [[ -f "$deb" && ! -L "$deb" ]] || fail "deb must be one regular file"

  local package_name architecture dependencies
  package_name="$(dpkg-deb --field "$deb" Package)"
  architecture="$(dpkg-deb --field "$deb" Architecture)"
  dependencies="$(dpkg-deb --field "$deb" Depends)"
  [[ "$package_name" == "$EXPECTED_PACKAGE_NAME" ]] || fail "unexpected deb package name: $package_name"
  [[ "$architecture" == amd64 ]] || fail "unexpected deb architecture: $architecture"
  grep -q 'libwebkit2gtk-4.1-0' <<<"$dependencies" || fail "deb does not declare the WebKitGTK 4.1 runtime"
  grep -q 'libgtk-3-0' <<<"$dependencies" || fail "deb does not declare the GTK 3 runtime"
  assert_no_forbidden_dependencies deb "$dependencies"

  local control_root="$work_root/deb-control"
  local payload_root="$work_root/deb-payload"
  mkdir "$control_root" "$payload_root"
  dpkg-deb --control "$deb" "$control_root"
  for control_script in preinst postinst prerm postrm config triggers; do
    [[ ! -e "$control_root/$control_script" ]] || fail "deb unexpectedly contains maintainer control: $control_script"
  done
  [[ -f "$control_root/md5sums" ]] || fail "deb is missing its payload checksum manifest"
  dpkg-deb --extract "$deb" "$payload_root"
  (cd "$payload_root" && md5sum --check "$control_root/md5sums")
  assert_application_tree "$payload_root"
}

verify_rpm() {
  local rpm_path
  rpm_path="$(realpath "$1")"
  [[ -f "$rpm_path" && ! -L "$rpm_path" ]] || fail "rpm must be one regular file"

  local package_name architecture dependencies scripts triggers signature_info
  package_name="$(rpm --query --package --queryformat '%{NAME}' "$rpm_path")"
  architecture="$(rpm --query --package --queryformat '%{ARCH}' "$rpm_path")"
  dependencies="$(rpm --query --package --requires "$rpm_path")"
  scripts="$(rpm --query --package --scripts "$rpm_path")"
  triggers="$(rpm --query --package --triggers "$rpm_path")"
  signature_info="$(rpm --checksig --verbose "$rpm_path")"

  [[ "$package_name" == "$EXPECTED_PACKAGE_NAME" ]] || fail "unexpected rpm package name: $package_name"
  [[ "$architecture" == x86_64 ]] || fail "unexpected rpm architecture: $architecture"
  grep -q 'libwebkit2gtk-4\.1\.so\.0' <<<"$dependencies" || fail "rpm does not require the WebKitGTK 4.1 runtime"
  grep -q 'libgtk-3\.so\.0' <<<"$dependencies" || fail "rpm does not require the GTK 3 runtime"
  assert_no_forbidden_dependencies rpm "$dependencies"
  [[ -z "$scripts" ]] || fail "rpm unexpectedly contains install or removal scriptlets"
  [[ -z "$triggers" ]] || fail "rpm unexpectedly contains trigger scriptlets"
  grep -q 'digest.*OK' <<<"$signature_info" || fail "rpm payload digest validation failed"
  if grep -qi 'Signature' <<<"$signature_info"; then
    fail "rpm is signed even though the release policy declares Linux artifacts unsigned"
  fi

  local payload_root="$work_root/rpm-payload"
  mkdir "$payload_root"
  rpm2cpio "$rpm_path" | (cd "$payload_root" && cpio --extract --make-directories --quiet --no-absolute-filenames)
  assert_application_tree "$payload_root"
}

smoke_appimage() {
  echo "Smoke testing the AppImage on Ubuntu 22.04..."
  timeout --signal=TERM --kill-after=5s 30s \
    bash "$(dirname "$0")/linux-gui-smoke.sh" "$1"
}

smoke_deb() {
  local deb="$1"
  local package_name="$EXPECTED_PACKAGE_NAME"
  local package_status
  package_status="$(dpkg-query --show --showformat='${db:Status-Abbrev}' "$package_name" 2>/dev/null || true)"
  [[ "$package_status" != ii* ]] || fail "deb smoke runner already has $package_name installed"

  echo "Installing, launching, and uninstalling the deb on Ubuntu 22.04..."
  deb_package_to_remove="$package_name"
  sudo env DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends "$deb"
  [[ "$(dpkg-query --show --showformat='${db:Status-Abbrev}' "$package_name")" == ii* ]] || fail "deb was not installed"
  [[ -x "$EXPECTED_EXECUTABLE" ]] || fail "deb did not install $EXPECTED_EXECUTABLE"

  local verification_output
  verification_output="$(sudo dpkg --verify "$package_name")"
  [[ -z "$verification_output" ]] || {
    echo "$verification_output" >&2
    fail "installed deb payload differs from its checksum manifest"
  }

  timeout --signal=TERM --kill-after=5s 30s \
    bash "$(dirname "$0")/linux-gui-smoke.sh" "$EXPECTED_EXECUTABLE"
  sudo env DEBIAN_FRONTEND=noninteractive apt-get remove --yes "$package_name"
  deb_package_to_remove=""
  [[ ! -e "$EXPECTED_EXECUTABLE" ]] || fail "deb uninstall left its application executable behind"
  if [[ "$(dpkg-query --show --showformat='${db:Status-Abbrev}' "$package_name" 2>/dev/null || true)" == ii* ]]; then
    fail "deb package remains installed after removal"
  fi
}

smoke_deb_container() {
  local deb_path repo_root deb_relative container_deb
  deb_path="$(realpath "$1")"
  repo_root="$(git rev-parse --show-toplevel)"
  repo_root="$(realpath "$repo_root")"
  [[ "$deb_path" == "$repo_root/"* ]] || fail "deb must be located inside the checked-out repository"
  deb_relative="${deb_path#"$repo_root"/}"
  container_deb="/workspace/$deb_relative"

  [[ -n "${DEBIAN_SMOKE_IMAGE:-}" ]] || fail "DEBIAN_SMOKE_IMAGE must pin the Debian smoke image"
  [[ "$DEBIAN_SMOKE_IMAGE" == *@sha256:* ]] || fail "DEBIAN_SMOKE_IMAGE must be pinned by digest"

  echo "Installing, launching, and uninstalling the deb in an ephemeral Debian ${DEBIAN_VERSION} container..."
  docker run --rm \
    --pull always \
    --platform linux/amd64 \
    --pids-limit 512 \
    --volume "$repo_root:/workspace:ro" \
    "$DEBIAN_SMOKE_IMAGE" \
    bash /workspace/.github/scripts/linux-package-smoke.sh debian-container "$container_deb"
}

run_deb_container_smoke() {
  local deb_path="$1"

  [[ -r /etc/os-release ]] || fail "Debian container lacks /etc/os-release"
  # shellcheck disable=SC1091
  source /etc/os-release
  [[ "${ID:-}" == debian && "${VERSION_ID:-}" == "$DEBIAN_VERSION" ]] || {
    fail "deb smoke requires Debian ${DEBIAN_VERSION}, found ${ID:-unknown} ${VERSION_ID:-unknown}"
  }

  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install --yes --no-install-recommends \
    binutils \
    ca-certificates \
    dbus-x11 \
    desktop-file-utils \
    file \
    findutils \
    passwd \
    procps \
    util-linux \
    xdotool \
    xvfb

  for command_name in apt-get dbus-run-session desktop-file-validate dpkg-deb dpkg-query file find md5sum readelf realpath runuser setsid timeout useradd Xvfb xdotool; do
    require_command "$command_name"
  done

  verify_deb "$deb_path"
  if [[ "$(dpkg-query --show --showformat='${db:Status-Abbrev}' "$EXPECTED_PACKAGE_NAME" 2>/dev/null || true)" == ii* ]]; then
    fail "Debian smoke container unexpectedly starts with $EXPECTED_PACKAGE_NAME installed"
  fi

  deb_package_to_remove="$EXPECTED_PACKAGE_NAME"
  apt-get install --yes --no-install-recommends "$deb_path"
  [[ "$(dpkg-query --show --showformat='${db:Status-Abbrev}' "$EXPECTED_PACKAGE_NAME")" == ii* ]] || fail "deb was not installed on Debian"
  [[ -x "$EXPECTED_EXECUTABLE" ]] || fail "deb did not install $EXPECTED_EXECUTABLE on Debian"

  local verification_output
  verification_output="$(dpkg --verify "$EXPECTED_PACKAGE_NAME")"
  [[ -z "$verification_output" ]] || {
    echo "$verification_output" >&2
    fail "Debian-installed deb payload differs from its checksum manifest"
  }

  useradd --create-home --shell /bin/bash aseprite-smoke
  runuser --user aseprite-smoke -- \
    env HOME=/home/aseprite-smoke \
    timeout --signal=TERM --kill-after=5s 30s \
    bash /workspace/.github/scripts/linux-gui-smoke.sh "$EXPECTED_EXECUTABLE"

  apt-get remove --yes "$EXPECTED_PACKAGE_NAME"
  deb_package_to_remove=""
  [[ ! -e "$EXPECTED_EXECUTABLE" ]] || fail "deb uninstall left its application executable behind on Debian"
  if [[ "$(dpkg-query --show --showformat='${db:Status-Abbrev}' "$EXPECTED_PACKAGE_NAME" 2>/dev/null || true)" == ii* ]]; then
    fail "deb package remains installed after Debian removal"
  fi
}

smoke_rpm_container() {
  local rpm_path repo_root rpm_relative container_rpm
  rpm_path="$(realpath "$1")"
  repo_root="$(git rev-parse --show-toplevel)"
  repo_root="$(realpath "$repo_root")"
  [[ "$rpm_path" == "$repo_root/"* ]] || fail "rpm must be located inside the checked-out repository"
  rpm_relative="${rpm_path#"$repo_root"/}"
  container_rpm="/workspace/$rpm_relative"

  [[ -n "${FEDORA_SMOKE_IMAGE:-}" ]] || fail "FEDORA_SMOKE_IMAGE must pin the Fedora smoke image"
  [[ "$FEDORA_SMOKE_IMAGE" == *@sha256:* ]] || fail "FEDORA_SMOKE_IMAGE must be pinned by digest"

  echo "Installing, launching, and uninstalling the rpm in an ephemeral Fedora ${FEDORA_VERSION} container..."
  docker run --rm \
    --pull always \
    --platform linux/amd64 \
    --pids-limit 512 \
    --volume "$repo_root:/workspace:ro" \
    "$FEDORA_SMOKE_IMAGE" \
    bash /workspace/.github/scripts/linux-package-smoke.sh rpm-container "$container_rpm"
}

run_rpm_container_smoke() {
  local rpm_path="$1"

  [[ -r /etc/os-release ]] || fail "Fedora container lacks /etc/os-release"
  # shellcheck disable=SC1091
  source /etc/os-release
  [[ "${ID:-}" == fedora && "${VERSION_ID:-}" == "$FEDORA_VERSION" ]] || {
    fail "rpm smoke requires Fedora ${FEDORA_VERSION}, found ${ID:-unknown} ${VERSION_ID:-unknown}"
  }

  dnf5 --assumeyes --setopt=install_weak_deps=False install \
    binutils \
    cpio \
    dbus-daemon \
    desktop-file-utils \
    file \
    findutils \
    procps-ng \
    rpm \
    shadow-utils \
    util-linux \
    xdotool \
    xorg-x11-server-Xvfb

  for command_name in cpio dbus-run-session desktop-file-validate dnf5 file find readelf realpath rpm rpm2cpio runuser setsid timeout useradd Xvfb xdotool; do
    require_command "$command_name"
  done

  verify_rpm "$rpm_path"
  if rpm --query "$EXPECTED_PACKAGE_NAME" >/dev/null 2>&1; then
    fail "Fedora smoke container unexpectedly starts with $EXPECTED_PACKAGE_NAME installed"
  fi

  rpm_package_to_remove="$EXPECTED_PACKAGE_NAME"
  dnf5 --assumeyes --no-gpgchecks --setopt=install_weak_deps=False install "$rpm_path"
  rpm --query "$EXPECTED_PACKAGE_NAME" >/dev/null || fail "rpm was not installed"
  [[ -x "$EXPECTED_EXECUTABLE" ]] || fail "rpm did not install $EXPECTED_EXECUTABLE"

  local verification_output
  verification_output="$(rpm --verify "$EXPECTED_PACKAGE_NAME")"
  [[ -z "$verification_output" ]] || {
    echo "$verification_output" >&2
    fail "installed rpm payload differs from its package manifest"
  }

  useradd --create-home --shell /bin/bash aseprite-smoke
  runuser --user aseprite-smoke -- \
    env HOME=/home/aseprite-smoke \
    timeout --signal=TERM --kill-after=5s 30s \
    bash /workspace/.github/scripts/linux-gui-smoke.sh "$EXPECTED_EXECUTABLE"

  dnf5 --assumeyes remove "$EXPECTED_PACKAGE_NAME"
  rpm_package_to_remove=""
  if rpm --query "$EXPECTED_PACKAGE_NAME" >/dev/null 2>&1; then
    fail "rpm package remains installed after removal"
  fi
  [[ ! -e "$EXPECTED_EXECUTABLE" ]] || fail "rpm uninstall left its application executable behind"
}

case "$mode" in
  all)
    [[ "$#" -eq 3 ]] || fail "usage: $0 all <AppImage> <deb> <rpm>"
    [[ -r /etc/os-release ]] || fail "Ubuntu smoke runner lacks /etc/os-release"
    # shellcheck disable=SC1091
    source /etc/os-release
    [[ "${ID:-}" == ubuntu && "${VERSION_ID:-}" == 22.04 ]] || {
      fail "host package smoke requires Ubuntu 22.04, found ${ID:-unknown} ${VERSION_ID:-unknown}"
    }
    for command_name in cpio dbus-run-session desktop-file-validate docker dpkg-deb file find git md5sum readelf realpath rpm rpm2cpio setsid sudo timeout Xvfb xdotool; do
      require_command "$command_name"
    done
    verify_appimage "$1"
    verify_deb "$2"
    verify_rpm "$3"
    smoke_appimage "$1"
    smoke_deb "$(realpath "$2")"
    smoke_deb_container "$2"
    smoke_rpm_container "$3"
    ;;
  debian-container)
    [[ "$#" -eq 1 ]] || fail "usage: $0 debian-container <deb>"
    run_deb_container_smoke "$1"
    ;;
  rpm-container)
    [[ "$#" -eq 1 ]] || fail "usage: $0 rpm-container <rpm>"
    run_rpm_container_smoke "$1"
    ;;
  *)
    fail "unknown mode '$mode' (expected 'all', 'debian-container', or 'rpm-container')"
    ;;
esac
