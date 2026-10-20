#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "==> Building rclip (release)..."
cargo build --release -p rclip

BIN_PATH="$SCRIPT_DIR/target/release/rclip"
INSTALL_BIN="$HOME/.local/bin"
mkdir -p "$INSTALL_BIN"
cp "$BIN_PATH" "$INSTALL_BIN/rclip"
echo "==> Installed binary to $INSTALL_BIN/rclip"

# D-Bus service & desktop file
mkdir -p "$HOME/.local/share/applications"
mkdir -p "$HOME/.local/share/dbus-1/services"

# Update Exec path in service file to point to $HOME/.local/bin/rclip
sed "s|Exec=.*|Exec=$HOME/.local/bin/rclip daemon|" "$SCRIPT_DIR/io.github.rclip.service" > "$HOME/.local/share/dbus-1/services/io.github.rclip.service"
cp "$SCRIPT_DIR/io.github.rclip.desktop" "$HOME/.local/share/applications/io.github.rclip.desktop"
echo "==> Installed D-Bus activation service and desktop entry"

# GNOME Shell extension for background clipboard capture on Wayland
EXT_DIR="$HOME/.local/share/gnome-shell/extensions/rclip-watcher@utilities.local"
mkdir -p "$EXT_DIR"
cp -r "$SCRIPT_DIR/gnome-extension/"* "$EXT_DIR/"
echo "==> Installed GNOME Shell watcher extension to $EXT_DIR"

# Configure GNOME shortcut for Super+V
if command -v gsettings >/dev/null 2>&1; then
    CUSTOM_KEY="/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom1/"
    gsettings set "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$CUSTOM_KEY" name 'rclip Clipboard'
    gsettings set "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$CUSTOM_KEY" command "$HOME/.local/bin/rclip toggle"
    gsettings set "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$CUSTOM_KEY" binding '<Super>v'
    echo "==> Configured <Super>+V shortcut to '$HOME/.local/bin/rclip toggle'"
fi

echo "==> Installation complete!"
