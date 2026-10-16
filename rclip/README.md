# rclip — Lightweight Wayland Clipboard Manager

`rclip` is an ultra-lightweight, memory-efficient clipboard history manager written in Rust with a native GTK4 floating card interface designed for GNOME on Wayland (Fedora).

## Key Features
- **Ultra-compact binary**: Stripped release binary with LTO (`opt-level = "z"`).
- **Fast D-Bus Activation**: Seamlessly activated via standard D-Bus actions with instant Wayland window focus forwarding.
- **Wayland Background Capture**: Captures clipboard updates across all applications using an integrated GNOME Shell extension and GdkClipboard watcher.
- **Privacy & Password Protection**: Automatically skips clips marked with `x-kde-passwordManagerHint` (e.g. 1Password, Bitwarden, KeePassXC).
- **Secure Persistence**: Atomic file writes with `0o700` directory and `0o600` file permissions and `fsync`.
- **Pinned History**: Pin favorite clips with <kbd>Ctrl</kbd>+<kbd>P</kbd>; pinned clips appear first and are never pruned.
- **Fuzzy Search**: Real-time fuzzy filtering powered by `nucleo-matcher` with substring fallback.

## Keyboard Controls
| Shortcut | Action |
|---|---|
| <kbd>Super</kbd> + <kbd>V</kbd> | Toggle clipboard popup at cursor / center |
| <kbd>↑</kbd> / <kbd>↓</kbd> | Navigate clipboard items |
| <kbd>Enter</kbd> | Copy selected item to clipboard and close window |
| <kbd>Ctrl</kbd> + <kbd>P</kbd> | Toggle pin on highlighted item |
| <kbd>Ctrl</kbd> + <kbd>D</kbd> | Delete highlighted item from history |
| <kbd>Escape</kbd> | Close popup without copying |

## CLI Commands
```bash
rclip toggle          # Toggle popup visibility (default)
rclip show            # Open popup
rclip hide            # Close popup
rclip clear           # Clear all unpinned history
rclip daemon          # Run in background daemon service mode
rclip add-clip "text" # Add text directly to history
```

## Configuration
`~/.config/rclip/config.json`:
```json
{
  "max_items": 100,
  "max_clip_bytes": 1000000
}
```

## Installation
```bash
./install.sh
```
Builds the release binary, installs it to `~/.local/bin/rclip`, installs the D-Bus service & desktop file, copies the GNOME Shell extension, and binds <kbd>Super</kbd>+<kbd>V</kbd>.
