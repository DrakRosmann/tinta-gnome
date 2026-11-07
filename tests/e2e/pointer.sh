#!/bin/bash
# Runs Tinta in a nested cosmic-comp inside a rootful Xwayland, so tinta-xptr can give it a real
# pointer. Its own window on the desktop, its own config; the desktop's cursor doesn't move.
#
#   tests/e2e/pointer.sh SCRIPT POINTER_STEPS [APP ARGS...]
#
# SCRIPT drives the app (crates/tinta/src/app/script.rs), POINTER_STEPS the pointer
# (crates/tinta-xptr/src/main.rs).
set -u
root=$(cd "$(dirname "$0")/../.." && pwd)
work=${TINTA_E2E_DIR:-$(mktemp -d)}
script=$1
steps=$2
shift 2
export XDG_CONFIG_HOME=$work/cfg XDG_STATE_HOME=$work/state XDG_CACHE_HOME=$work/cache
mkdir -p "$XDG_CONFIG_HOME" "$XDG_STATE_HOME"

disp=:$(( 70 + RANDOM % 20 ))
Xwayland "$disp" -geometry 1280x800 -noreset > "$work/xwayland.log" 2>&1 &
xw=$!
trap 'kill $comp $xw 2>/dev/null' EXIT
comp=
for _ in $(seq 1 50); do [ -S "/tmp/.X11-unix/X${disp#:}" ] && break; sleep 0.1; done
sleep 0.3

touch "$work/stamp"
sleep 0.05
DISPLAY=$disp RUST_LOG=error cosmic-comp > "$work/comp.log" 2>&1 &
comp=$!
sock=
for _ in $(seq 1 80); do
    for f in "$XDG_RUNTIME_DIR"/wayland-*; do
        [ -S "$f" ] && [ "$f" -nt "$work/stamp" ] && sock=$(basename "$f")
    done
    [ -n "$sock" ] && break
    sleep 0.1
done
[ -n "$sock" ] || { echo "no nested compositor:"; tail -5 "$work/comp.log"; exit 1; }

(
    export WAYLAND_DISPLAY=$sock
    unset DISPLAY
    TINTA_SCRIPT=$script exec dbus-run-session -- timeout "${TINTA_TIMEOUT:-60}" \
        "$root/target/debug/tinta-gnome" "$@"
) > "$work/app.log" 2>&1 &
app=$!
DISPLAY=$disp "$root/target/debug/tinta-xptr" COSMIC < "$steps" || echo "tinta-xptr failed"
wait $app
status=$?
grep -E 'panicked' "$work/app.log"
exit $status
