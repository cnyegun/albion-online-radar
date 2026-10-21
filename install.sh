#!/usr/bin/env bash
# Builds OpenRadar and installs it for the current user:
#   ~/.local/bin/openradar                 (with the capture capability)
#   ~/.local/share/openradar/maps/         map backgrounds
#   ~/.local/share/applications/openradar.desktop and its icon
#
#   ./install.sh              build and install (asks for sudo once, for setcap)
#   ./install.sh --uninstall  remove all of the above (settings are kept)
set -euo pipefail
cd "$(dirname "$0")"

bin="$HOME/.local/bin/openradar"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
desktop="$data/applications/openradar.desktop"
icon="$data/icons/hicolor/scalable/apps/openradar.svg"

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$bin" "$desktop" "$icon"
    rm -rf "$data/openradar"
    echo "Removed. Settings remain in ~/.config/openradar."
    exit 0
fi

cargo build --release

if pgrep -x openradar >/dev/null; then
    echo "Quit the running radar first (Alt+Tab to it, then Ctrl+Q)." >&2
    exit 1
fi

install -Dm755 target/release/openradar "$bin"
mkdir -p "$data/openradar/maps"
cp -u assets/maps/*.webp "$data/openradar/maps/"
install -Dm644 assets/openradar.svg "$icon"
mkdir -p "$(dirname "$desktop")"
sed "s|@BIN@|$bin|" assets/openradar.desktop >"$desktop"
update-desktop-database -q "$data/applications" 2>/dev/null || true
gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true

# Lets the radar capture packets without running as root.
sudo setcap cap_net_raw=eip "$bin"

echo "Installed. Launch OpenRadar from the app grid."
