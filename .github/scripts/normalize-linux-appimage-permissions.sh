#!/usr/bin/env bash

set -euo pipefail

fail() {
  echo "AppImage permission normalization failed: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "missing command: $1"
}

[[ "$#" -eq 2 ]] || fail "usage: $0 <AppImage> <AppDir>"

for required_command in basename chmod dirname find grep mkdir mktemp mv realpath rm; do
  require_command "$required_command"
done

[[ -f "$1" && ! -L "$1" && -x "$1" ]] || \
  fail "input must be one executable regular AppImage"
[[ -d "$2" && ! -L "$2" && -f "$2/AppRun" && ! -L "$2/AppRun" && -x "$2/AppRun" ]] || \
  fail "input must be one AppDir with an executable AppRun"
appimage="$(realpath "$1")"
app_dir="$(realpath "$2")"
[[ "$appimage" == *.AppImage && "$app_dir" == *.AppDir ]] || \
  fail "inputs must use the AppImage and AppDir suffixes"
[[ "$(dirname "$appimage")" == "$(dirname "$app_dir")" ]] || \
  fail "the AppImage and AppDir must share their bundle directory"

cache_root="${XDG_CACHE_HOME:-${HOME}/.cache}/tauri"
linuxdeploy="$cache_root/linuxdeploy-x86_64.AppImage"
[[ -f "$linuxdeploy" && ! -L "$linuxdeploy" && -x "$linuxdeploy" ]] || \
  fail "Tauri's cached x86_64 linuxdeploy tool is unavailable"

while IFS= read -r -d '' writable_file; do
  chmod go-w -- "$writable_file"
done < <(find "$app_dir" -xdev -type f -perm /0022 -print0)

if find "$app_dir" -xdev -type f -perm /0022 -print -quit | grep -q .; then
  fail "a group- or world-writable regular file remains in the AppDir"
fi

appimage_directory="$(dirname "$appimage")"
appimage_name="$(basename "$appimage")"
work_root="$(mktemp -d "$appimage_directory/.appimage-normalize.XXXXXX")"

cleanup() {
  if [[ -d "$work_root" ]]; then
    rm -r -- "$work_root"
  fi
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

normalized_path="$work_root/$appimage_name"
OUTPUT="$normalized_path" \
ARCH=x86_64 \
APPIMAGE_EXTRACT_AND_RUN=1 \
  "$linuxdeploy" \
    --appimage-extract-and-run \
    --verbosity 1 \
    --appdir "$app_dir" \
    --output appimage

[[ -f "$normalized_path" && ! -L "$normalized_path" ]] || \
  fail "linuxdeploy did not produce the normalized AppImage"
chmod 0755 "$normalized_path"

verify_root="$work_root/verify"
mkdir "$verify_root"
(
  cd "$verify_root"
  "$normalized_path" --appimage-extract >/dev/null
)
[[ -x "$verify_root/squashfs-root/AppRun" ]] || \
  fail "the normalized AppImage cannot be extracted"
if find "$verify_root/squashfs-root" -xdev -type f -perm /0022 -print -quit | grep -q .; then
  fail "the normalized AppImage still contains a group- or world-writable regular file"
fi

mv -f -- "$normalized_path" "$appimage"
