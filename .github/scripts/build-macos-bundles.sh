#!/usr/bin/env bash

set -euo pipefail

if (( $# != 1 )); then
  echo "Usage: $0 <aarch64-apple-darwin|x86_64-apple-darwin>" >&2
  exit 64
fi

build_target="$1"
case "$build_target" in
  aarch64-apple-darwin | x86_64-apple-darwin) ;;
  *)
    echo "Unsupported macOS build target: $build_target" >&2
    exit 64
    ;;
esac

# Keep compilation separate so a transient macOS disk-image failure can be
# retried without rebuilding the application.
npm run tauri build -- --ci --target "$build_target" --no-bundle -- --locked

maximum_attempts=3
for (( attempt = 1; attempt <= maximum_attempts; attempt++ )); do
  echo "Creating macOS app and DMG bundles (attempt $attempt/$maximum_attempts)."
  if npm run tauri bundle -- --ci --verbose --target "$build_target" --bundles app,dmg; then
    exit 0
  fi

  if (( attempt == maximum_attempts )); then
    echo "macOS bundle creation failed after $maximum_attempts attempts." >&2
    exit 1
  fi

  retry_delay_seconds=$(( attempt * 10 ))
  echo "macOS bundle creation failed; retrying in $retry_delay_seconds seconds." >&2
  sleep "$retry_delay_seconds"
done
