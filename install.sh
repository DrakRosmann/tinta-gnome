#!/bin/sh
# Build and install for the current user (default) or under another prefix:
#   ./install.sh            -> ~/.local
#   sudo ./install.sh /usr  -> system wide
#   ./install.sh /app       -> in a Flatpak build
set -e
cd "$(dirname "$0")"
prefix="${1:-$HOME/.local}"
id=io.github.DrakRosmann.TintaGnome
bin=tinta-gnome

# In a Flatpak build the manifest brings what's needed.
[ -f /.flatpak-info ] || ./deps.sh
cargo build --release -p tinta-gnome
install -Dm0755 "target/release/$bin" "$prefix/bin/$bin"
# The original Tinta owns the short `tinta` name; this one keeps its own.
mkdir -p "$prefix/share/applications"
sed "s|@bindir@|$prefix/bin|" "data/$id.desktop.in" > "$prefix/share/applications/$id.desktop"
install -Dm0644 "data/$id.metainfo.xml" "$prefix/share/metainfo/$id.metainfo.xml"
install -Dm0644 "data/icons/scalable/apps/$id.svg" "$prefix/share/icons/hicolor/scalable/apps/$id.svg"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$prefix/share/applications" || true
echo "Instalado. Abra \"Tinta (GTK)\" pelo lançador de aplicativos, ou rode: tinta-gnome [arquivo ou pasta]"
