#!/usr/bin/env bash

set -euo pipefail

readonly EXPECTED_WINDOW_TITLE="Aseprite Installer"

fail() {
  echo "Linux GUI smoke test failed: $*" >&2
  exit 1
}

if [[ "$#" -lt 1 ]]; then
  fail "expected an executable and optional arguments"
fi

for command_name in Xvfb dbus-run-session setsid xdotool; do
  command -v "$command_name" >/dev/null 2>&1 || fail "missing command: $command_name"
done

smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/aseprite-installer-gui-smoke.XXXXXX")"
xvfb_pid=""
app_pid=""

terminate_process_group() {
  local process_group="$1"

  kill -TERM -- "-${process_group}" >/dev/null 2>&1 || true
  for _ in $(seq 1 20); do
    if ! kill -0 -- "-${process_group}" >/dev/null 2>&1; then
      return
    fi
    sleep 0.1
  done
  kill -KILL -- "-${process_group}" >/dev/null 2>&1 || true
}

cleanup() {
  if [[ -n "$app_pid" ]]; then
    terminate_process_group "$app_pid"
    wait "$app_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$xvfb_pid" ]]; then
    kill -TERM "$xvfb_pid" >/dev/null 2>&1 || true
    wait "$xvfb_pid" >/dev/null 2>&1 || true
  fi
  if [[ -d "$smoke_root" ]]; then
    rm -r -- "$smoke_root"
  fi
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir "$smoke_root/xdg-runtime"
chmod 0700 "$smoke_root/xdg-runtime"
export XDG_RUNTIME_DIR="$smoke_root/xdg-runtime"
export GDK_BACKEND=x11
export LIBGL_ALWAYS_SOFTWARE=1
export NO_AT_BRIDGE=1
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1
unset DBUS_SESSION_BUS_ADDRESS
unset WAYLAND_DISPLAY

display_number=""
for candidate in $(seq 90 119); do
  if [[ -e "/tmp/.X11-unix/X${candidate}" || -e "/tmp/.X${candidate}-lock" ]]; then
    continue
  fi

  Xvfb ":${candidate}" \
    -screen 0 1280x720x24 \
    -nolisten tcp \
    -ac \
    >"$smoke_root/xvfb.log" 2>&1 &
  xvfb_pid="$!"

  for _ in $(seq 1 40); do
    if [[ -S "/tmp/.X11-unix/X${candidate}" ]]; then
      display_number="$candidate"
      break
    fi
    if ! kill -0 "$xvfb_pid" >/dev/null 2>&1; then
      wait "$xvfb_pid" >/dev/null 2>&1 || true
      xvfb_pid=""
      break
    fi
    sleep 0.1
  done

  if [[ -n "$display_number" ]]; then
    break
  fi
done

if [[ -z "$display_number" || -z "$xvfb_pid" ]]; then
  sed -n '1,200p' "$smoke_root/xvfb.log" >&2 || true
  fail "could not start an isolated X server"
fi
export DISPLAY=":${display_number}"

setsid dbus-run-session -- "$@" >"$smoke_root/application.log" 2>&1 &
app_pid="$!"

window_seen=false
for _ in $(seq 1 60); do
  if ! kill -0 "$app_pid" >/dev/null 2>&1; then
    set +e
    wait "$app_pid"
    app_status="$?"
    set -e
    app_pid=""
    sed -n '1,200p' "$smoke_root/application.log" >&2 || true
    fail "application exited before exposing its main window (status ${app_status})"
  fi

  if xdotool search --onlyvisible --name "^${EXPECTED_WINDOW_TITLE}$" >/dev/null 2>&1; then
    window_seen=true
    break
  fi
  sleep 0.25
done

if [[ "$window_seen" != true ]]; then
  sed -n '1,200p' "$smoke_root/application.log" >&2 || true
  fail "no visible '${EXPECTED_WINDOW_TITLE}' window appeared within 15 seconds"
fi

# A mapped window alone can be transient. Keep the app alive briefly so early
# WebKit, IPC, and native initialization failures still fail the smoke test.
sleep 2
if ! kill -0 "$app_pid" >/dev/null 2>&1; then
  set +e
  wait "$app_pid"
  app_status="$?"
  set -e
  app_pid=""
  sed -n '1,200p' "$smoke_root/application.log" >&2 || true
  fail "application exited immediately after mapping its main window (status ${app_status})"
fi

echo "Linux GUI smoke test passed: a stable '${EXPECTED_WINDOW_TITLE}' window was mapped under Xvfb."
