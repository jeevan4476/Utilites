#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EXT_UUID="rclip@utilities.local"
EXT_DIR="${HOME}/.local/share/gnome-shell/extensions/${EXT_UUID}"
OLD_EXT_DIR="${HOME}/.local/share/gnome-shell/extensions/rclip-watcher@utilities.local"
INSTALL_BIN="${HOME}/.local/bin"

echo "==> Installing rclip CLI to ${INSTALL_BIN}/rclip..."
mkdir -p "$INSTALL_BIN"
cat > "${INSTALL_BIN}/rclip" <<EOF
#!/usr/bin/env bash
PYTHONPATH="${SCRIPT_DIR}\${PYTHONPATH:+:\$PYTHONPATH}" exec python3 -m rclip_cli.cli "\$@"
EOF
chmod +x "${INSTALL_BIN}/rclip"

echo "==> Cleaning up legacy rclip daemon / watcher artifacts..."
if command -v gnome-extensions >/dev/null 2>&1; then
    gnome-extensions disable rclip-watcher@utilities.local >/dev/null 2>&1 || true
fi
rm -rf "$OLD_EXT_DIR"
rm -f "${HOME}/.local/share/dbus-1/services/io.github.rclip.service"
rm -f "${HOME}/.local/share/applications/io.github.rclip.desktop"

# Remove legacy custom1 <Super>v keybinding created by v1 if it points to rclip
if command -v gsettings >/dev/null 2>&1; then
    CUSTOM_KEY="/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom1/"
    EXISTING_CMD="$(gsettings get "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:${CUSTOM_KEY}" command 2>/dev/null || true)"
    if [[ "$EXISTING_CMD" == *rclip* ]]; then
        gsettings reset "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:${CUSTOM_KEY}" name || true
        gsettings reset "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:${CUSTOM_KEY}" command || true
        gsettings reset "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:${CUSTOM_KEY}" binding || true
        echo "==> Cleared legacy custom1 <Super>v shortcut"
    fi
fi

echo "==> Installing GNOME Shell extension (${EXT_UUID})..."
mkdir -p "$EXT_DIR"
cp -r "${SCRIPT_DIR}/extension/"* "$EXT_DIR/"

if command -v glib-compile-schemas >/dev/null 2>&1; then
    glib-compile-schemas "${EXT_DIR}/schemas"
else
    echo "Warning: glib-compile-schemas not found; extension settings schema was not compiled." >&2
fi

if command -v gnome-extensions >/dev/null 2>&1; then
    gnome-extensions enable "$EXT_UUID" >/dev/null 2>&1 || true
fi

echo "==> Installation complete!"
echo "    On Wayland, log out and log back in once so GNOME Shell loads '${EXT_UUID}'."
echo "    Then press Super+V anywhere to open and auto-paste."
