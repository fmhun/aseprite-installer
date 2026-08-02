#!/bin/sh

set -eu

repository_url="https://fmhun.github.io/aseprite-installer/apt"
archive_key_url="$repository_url/aseprite-installer-archive-keyring.asc"
archive_key_sha256="7da5b9ebe4474cb51938aea22691469825a495e897e872c3b13270e47de69efb"
sources_url="$repository_url/aseprite-installer.sources"
sources_sha256="b860716e24ae38ba5fdbf354590e66200863678ec99bcad75a990c05cf6db1ca"
preferences_url="$repository_url/aseprite-installer.pref"
preferences_sha256="84f57f37817bcc0d58f31d2c7328ca0a81f54f70eaad8860b4580d305fa42c4a"
archive_key_path="/etc/apt/keyrings/aseprite-installer-archive-keyring.asc"
sources_path="/etc/apt/sources.list.d/aseprite-installer.sources"
preferences_path="/etc/apt/preferences.d/aseprite-installer"

fail() {
  printf 'Aseprite Installer: %s\n' "$1" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

run_as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  else
    sudo "$@"
  fi
}

require_command apt-get
require_command curl
require_command dpkg
require_command id
require_command install
require_command mktemp
require_command sha256sum

if [ "$(id -u)" -ne 0 ]; then
  require_command sudo
fi

architecture="$(dpkg --print-architecture)"
[ "$architecture" = "amd64" ] || fail "Linux x86_64 (amd64) is required; detected $architecture"

if [ ! -r /etc/os-release ]; then
  fail "cannot identify this Linux distribution because /etc/os-release is unavailable"
fi

# shellcheck disable=SC1091
. /etc/os-release
distribution_family="${ID:-} ${ID_LIKE:-}"
case " $distribution_family " in
  *" debian "*|*" ubuntu "*) ;;
  *) fail "the signed APT repository supports Debian, Ubuntu, and compatible derivatives" ;;
esac

temporary_directory="$(mktemp -d "${TMPDIR:-/tmp}/aseprite-installer-apt.XXXXXX")"
cleanup() {
  if [ -n "${temporary_directory:-}" ] && [ -d "$temporary_directory" ]; then
    rm -r -- "$temporary_directory"
  fi
}
trap cleanup EXIT HUP INT TERM

downloaded_key="$temporary_directory/aseprite-installer-archive-keyring.asc"
temporary_sources="$temporary_directory/aseprite-installer.sources"
temporary_preferences="$temporary_directory/aseprite-installer.pref"

printf '%s\n' "Downloading the Aseprite Installer repository key…"
curl --fail --silent --show-error --location \
  --proto '=https' --proto-redir '=https' --tlsv1.2 \
  --output "$downloaded_key" \
  "$archive_key_url"

printf '%s  %s\n' "$archive_key_sha256" "$downloaded_key" | sha256sum --check --status \
  || fail "the repository key did not match its pinned SHA-256 digest"

printf '%s\n' "Downloading the scoped APT source definition…"
curl --fail --silent --show-error --location \
  --proto '=https' --proto-redir '=https' --tlsv1.2 \
  --output "$temporary_sources" \
  "$sources_url"

printf '%s  %s\n' "$sources_sha256" "$temporary_sources" | sha256sum --check --status \
  || fail "the repository source definition did not match its pinned SHA-256 digest"

printf '%s\n' "Downloading the package origin policy…"
curl --fail --silent --show-error --location \
  --proto '=https' --proto-redir '=https' --tlsv1.2 \
  --output "$temporary_preferences" \
  "$preferences_url"

printf '%s  %s\n' "$preferences_sha256" "$temporary_preferences" | sha256sum --check --status \
  || fail "the package origin policy did not match its pinned SHA-256 digest"

printf '%s\n' "Registering the signed APT repository…"
run_as_root install -d -m 0755 \
  /etc/apt/keyrings \
  /etc/apt/preferences.d \
  /etc/apt/sources.list.d
run_as_root install -m 0644 "$downloaded_key" "$archive_key_path"
run_as_root install -m 0644 "$temporary_sources" "$sources_path"
run_as_root install -m 0644 "$temporary_preferences" "$preferences_path"

printf '%s\n' "Refreshing APT and installing Aseprite Installer…"
run_as_root apt-get update
run_as_root apt-get install --yes aseprite-installer

printf '%s\n' "Aseprite Installer is installed. Future updates are handled by APT."
