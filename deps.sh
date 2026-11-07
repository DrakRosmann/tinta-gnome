#!/bin/sh
# Installs what building Tinta needs with the system's package manager (apt, dnf or pacman),
# if anything is missing, and checks that Rust is as new as the GTK bindings ask.
set -e

# What the GTK 4 bindings ask for.
rust=1.88

missing=
for tool in git cc pkg-config cargo; do
    command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
done
if command -v pkg-config >/dev/null 2>&1; then
    for lib in gtk4 libadwaita-1 vte-2.91-gtk4; do
        pkg-config --exists "$lib" || missing="$missing $lib"
    done
fi

if [ -n "$missing" ]; then
    echo "Missing:$missing"
    if command -v apt-get >/dev/null 2>&1; then
        sudo apt-get install -y git build-essential pkgconf cargo rustc libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev
    elif command -v dnf >/dev/null 2>&1; then
        sudo dnf install -y git gcc pkgconf-pkg-config cargo rust gtk4-devel libadwaita-devel vte291-gtk4-devel
    elif command -v pacman >/dev/null 2>&1; then
        rustpkg=rust
        # rustup provides cargo too; don't swap one for the other.
        command -v rustup >/dev/null 2>&1 && rustpkg=
        sudo pacman -S --needed --noconfirm git base-devel pkgconf $rustpkg gtk4 libadwaita vte4
    else
        echo "Unknown package manager: install these yourself." >&2
        exit 1
    fi
fi

# Distributions often ship an older Rust than the bindings ask for.
have=$(rustc --version | cut -d' ' -f2)
if [ "$(printf '%s\n%s\n' "$rust" "$have" | sort -V | head -n1)" != "$rust" ]; then
    if command -v rustup >/dev/null 2>&1; then
        rustup update stable
    else
        echo "Rust $have is too old: $rust or newer is needed. Install it with rustup" >&2
        echo "(https://rustup.rs), then run this again." >&2
        exit 1
    fi
fi
