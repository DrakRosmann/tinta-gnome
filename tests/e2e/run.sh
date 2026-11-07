#!/bin/bash
# Runs Tinta inside a nested cosmic-comp, with its own config, so the desktop in use and its
# settings are left alone. Real key presses come from tinta-vkbd through the compositor.
#
#   tests/e2e/run.sh SCRIPT [KEYS [LAYOUT]] [-- APP ARGS...]
#
# SCRIPT drives the app (see crates/tinta/src/app/script.rs); KEYS are typed by tinta-vkbd
# (see crates/tinta-vkbd/src/main.rs), on LAYOUT (default "us").
set -u
root=$(cd "$(dirname "$0")/../.." && pwd)
work=${TINTA_E2E_DIR:-$(mktemp -d)}
script=$1
keys=${2:-}
layout=${3:-us}
shift $(( $# < 3 ? $# : 3 ))
[ "${1:-}" = "--" ] && shift

export XDG_CONFIG_HOME=$work/cfg XDG_STATE_HOME=$work/state XDG_CACHE_HOME=$work/cache
mkdir -p "$XDG_CONFIG_HOME" "$XDG_STATE_HOME"
stamp=$work/stamp
touch "$stamp"
sleep 0.05
RUST_LOG=error cosmic-comp > "$work/comp.log" 2>&1 &
comp=$!
trap 'kill $comp 2>/dev/null' EXIT

# The new socket, and never the session's: it's the one newer than the stamp.
sock=
for _ in $(seq 1 80); do
    for f in "$XDG_RUNTIME_DIR"/wayland-*; do
        [ -S "$f" ] && [ "$f" -nt "$stamp" ] && sock=$(basename "$f")
    done
    [ -n "$sock" ] && break
    sleep 0.1
done
[ -n "$sock" ] || { echo "no nested compositor:"; tail -5 "$work/comp.log"; exit 1; }
export WAYLAND_DISPLAY=$sock
unset DISPLAY

TINTA_SCRIPT=$script dbus-run-session -- timeout "${TINTA_TIMEOUT:-60}" \
    "$root/target/debug/tinta-gnome" "$@" > "$work/app.log" 2>&1 &
app=$!
if [ -n "$keys" ]; then
    "$root/target/debug/tinta-vkbd" "$layout" < "$keys" || echo "tinta-vkbd failed"
fi
wait $app
status=$?
grep -E 'panicked' "$work/app.log"
exit $status
