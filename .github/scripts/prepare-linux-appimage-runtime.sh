#!/usr/bin/env bash

set -euo pipefail

readonly APPRUN_URL="https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-x86_64"
readonly APPRUN_SHA256="f30140a43a0a59e46db21bdefdf749b9e9f2c6946e92afabbacf98b8ae73fb4f"

cache_root="${XDG_CACHE_HOME:-${HOME}/.cache}/tauri"
runtime_path="$cache_root/AppRun-x86_64"
temporary_path=""

cleanup() {
  if [[ -n "$temporary_path" && -f "$temporary_path" ]]; then
    rm -f -- "$temporary_path"
  fi
}
trap cleanup EXIT

mkdir -p "$cache_root"
temporary_path="$(mktemp "$cache_root/.AppRun-x86_64.XXXXXX")"
curl --proto '=https' --tlsv1.2 --retry 5 --retry-connrefused \
  --location --silent --show-error --fail \
  "$APPRUN_URL" \
  --output "$temporary_path"
printf '%s  %s\n' "$APPRUN_SHA256" "$temporary_path" | sha256sum -c - >/dev/null
chmod 0755 "$temporary_path"
mv -f -- "$temporary_path" "$runtime_path"
temporary_path=""

runtime_mode="$(stat --format='%a' "$runtime_path" 2>/dev/null || stat -f '%Lp' "$runtime_path")"
[[ "$runtime_mode" == 755 ]] || {
  echo "Pinned AppRun does not have mode 0755." >&2
  exit 1
}
