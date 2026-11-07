#!/bin/bash
# Runs the editor inside a real GNOME Shell, headless, on a D-Bus of its own and with GNOME's
# settings in a file of its own: the session in use and its settings are left alone.
#
#   tests/e2e/gnome.sh SCRIPT [GSETTINGS...] [-- APP ARGS...]
#
# GSETTINGS are `key=value` for org.gnome.desktop.interface (`accent-color=teal`), or
# `wm:key=value` for org.gnome.desktop.wm.preferences. SCRIPT drives the editor, as in run.sh.
#
# The script's own `shot` draws the window but leaves the header bar's title out; for real pixels
# of the GNOME look, run pointer.sh with GSETTINGS_BACKEND=keyfile and `"interfaceStyle": "gnome"`.
set -u
root=$(cd "$(dirname "$0")/../.." && pwd)
work=${TINTA_E2E_DIR:-$(mktemp -d)}
script=$1
shift
prefs=()
while [ $# -gt 0 ] && [ "$1" != "--" ]; do prefs+=("$1"); shift; done
[ "${1:-}" = "--" ] && shift

export XDG_CONFIG_HOME=$work/cfg XDG_STATE_HOME=$work/state XDG_CACHE_HOME=$work/cache XDG_DATA_HOME=$work/data
export GSETTINGS_BACKEND=keyfile
mkdir -p "$XDG_CONFIG_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME" "$XDG_DATA_HOME"
for p in "${prefs[@]}"; do
    schema=org.gnome.desktop.interface
    case $p in wm:*) schema=org.gnome.desktop.wm.preferences; p=${p#wm:} ;; esac
    gsettings set "$schema" "${p%%=*}" "${p#*=}"
done

display=tinta-gnome-$$
cat > "$work/inner.sh" <<INNER
gnome-shell --headless --wayland --no-x11 --debug-control --virtual-monitor 1280x800 --wayland-display $display > "$work/shell.log" 2>&1 &
shell=\$!
for _ in \$(seq 1 150); do [ -S "\$XDG_RUNTIME_DIR/$display" ] && break; sleep 0.1; done
[ -S "\$XDG_RUNTIME_DIR/$display" ] || { echo "GNOME Shell did not start:"; tail -5 "$work/shell.log"; kill \$shell; exit 1; }
# GNOME_SHOT=SECONDS:FILE takes a picture of the whole screen, as GNOME Shell itself draws it.
if [ -n "\${GNOME_SHOT:-}" ]; then
    ( sleep "\${GNOME_SHOT%%:*}"
      gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell/DebugControl \\
          --method org.freedesktop.DBus.Properties.Set org.gnome.Shell.DebugControl UnsafeMode "<true>" > /dev/null 2>&1
      gdbus call --session --dest org.gnome.Shell.Screenshot --object-path /org/gnome/Shell/Screenshot \\
          --method org.gnome.Shell.Screenshot.Screenshot false false "\${GNOME_SHOT#*:}" > "$work/shot.log" 2>&1 ) &
fi
XDG_CURRENT_DESKTOP=GNOME WAYLAND_DISPLAY=$display TINTA_SCRIPT="$script" \\
    timeout "\${TINTA_TIMEOUT:-60}" "$root/target/debug/tinta-gnome" "\$@" > "$work/app.log" 2>&1
status=\$?
kill \$shell 2>/dev/null
wait \$shell 2>/dev/null
exit \$status
INNER
unset DISPLAY WAYLAND_DISPLAY
dbus-run-session -- bash "$work/inner.sh" "$@"
status=$?
grep -E 'panicked' "$work/app.log"
exit $status
