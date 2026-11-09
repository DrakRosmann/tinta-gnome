#!/bin/sh
# The header bar editor, clicked through AT-SPI: removing a button, moving one across and along a
# side, each checked in the settings it writes.
#
#   tests/e2e/headerbar.sh
#
# On a D-Bus of its own and with a runtime folder of its own, like a11y.sh. The window shows up
# on the session's display for a few seconds.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
app=${TINTA_APP:-$root/target/release/tinta-gnome}
libexec=/usr/lib
[ -x /usr/libexec/at-spi-bus-launcher ] && libexec=/usr/libexec
mkdir -m700 "$work/run"
printf 'wait 600\ncmd app.editHeaderBar\nwait 15000\nquit\n' > "$work/steps.txt"
cat > "$work/inner.sh" <<INNER
$libexec/at-spi-bus-launcher --launch-immediately >/dev/null 2>&1 &
sleep 1
$libexec/at-spi2-registryd --use-gnome-session=false >/dev/null 2>&1 &
sleep 1
LANG=C LC_ALL=C XDG_CONFIG_HOME=$work/cfg XDG_STATE_HOME=$work/state XDG_CACHE_HOME=$work/cache TINTA_SCRIPT=$work/steps.txt GTK_A11Y=atspi \
    timeout 30 "$app" --new-session >/dev/null 2>&1 &
sleep 2
python3 -I "$root/tests/e2e/headerbar.py" "$work/cfg"
INNER
display=${WAYLAND_DISPLAY:-wayland-0}
case $display in /*) ;; *) display=$XDG_RUNTIME_DIR/$display ;; esac
env XDG_RUNTIME_DIR="$work/run" WAYLAND_DISPLAY="$display" dbus-run-session -- sh "$work/inner.sh" 2>/dev/null
