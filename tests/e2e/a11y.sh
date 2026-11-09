#!/bin/sh
# What a screen reader gets from the text area, through AT-SPI as Orca reads it: the role, the
# text, a line, where a character is, and the events as the caret moves and text comes and goes.
#
#   tests/e2e/a11y.sh
#
# On a D-Bus of its own and with a runtime folder of its own, so the accessibility bus, and the
# portals the editor wakes, never touch the session in use. Prints what it saw and fails on what
# a screen reader would miss.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
app=${TINTA_APP:-$root/target/release/tinta-gnome}
# Arch keeps the AT-SPI daemons in /usr/lib, Fedora and Debian in /usr/libexec.
libexec=/usr/lib
[ -x /usr/libexec/at-spi-bus-launcher ] && libexec=/usr/libexec
mkdir -m700 "$work/run"
printf 'fn main(){\n    let a = 1;\n}\n' > "$work/a.rs"
printf 'wait 600\nopen %s/a.rs\nwait 3500\nkey Down\nwait 300\nkey End\nwait 300\ntype x\nwait 300\nkey BackSpace\nwait 3000\nquit\n' "$work" > "$work/steps.txt"
cat > "$work/inner.sh" <<INNER
$libexec/at-spi-bus-launcher --launch-immediately >/dev/null 2>&1 &
sleep 1
$libexec/at-spi2-registryd --use-gnome-session=false >/dev/null 2>&1 &
sleep 1
XDG_CONFIG_HOME=$work/cfg XDG_STATE_HOME=$work/state XDG_CACHE_HOME=$work/cache TINTA_SCRIPT=$work/steps.txt GTK_A11Y=atspi \
    timeout 30 "$app" --new-session >/dev/null 2>&1 &
sleep 1
python3 -I "$root/tests/e2e/a11y.py"
INNER
# The display stays the session's, by its full path; everything else is this run's own.
display=${WAYLAND_DISPLAY:-wayland-0}
case $display in /*) ;; *) display=$XDG_RUNTIME_DIR/$display ;; esac
env XDG_RUNTIME_DIR="$work/run" WAYLAND_DISPLAY="$display" dbus-run-session -- sh "$work/inner.sh" 2>/dev/null
