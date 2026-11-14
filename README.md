# Utilites

A collection of lightweight desktop utilities for Linux.

---

## rclip — Lightweight Wayland Clipboard Manager

`rclip` is an ultra-lightweight, memory-efficient clipboard history manager written in Rust with a native GTK4 floating card interface designed for GNOME on Wayland (Fedora).

### Key Features
- **Ultra-compact binary**: Stripped release binary is ~**470 KB** (linking against desktop system libraries).
- **Fast D-Bus Activation**: Seamlessly activated via standard D-Bus actions with instant Wayland window focus forwarding.
- **Wayland Background Capture**: Captures clipboard updates across all applications (terminal, browser, editors) using an integrated GNOME Shell extension and GdkClipboard watcher.
- **Privacy & Password Protection**: Automatically skips clips marked with `x-kde-passwordManagerHint` (e.g. 1Password, Bitwarden, KeePassXC).
- **Secure Persistence**: Atomic file writes with `0o700` directory and `0o600` file permissions and `fsync`.
- **Pinned History**: Pin favorite clips with <kbd>Ctrl</kbd>+<kbd>P</kbd>; pinned clips appear first and are never pruned.
- **Fuzzy Search**: Real-time fuzzy filtering powered by the `nucleo-matcher` crate with substring fallback.
- **Non-interfering Deletion**: Delete individual clips with <kbd>Ctrl</kbd>+<kbd>D</kbd> or the inline <kbd>✕</kbd> button (preserving the normal <kbd>Delete</kbd> key for text editing in the search box).

### Keyboard Controls
| Shortcut | Action |
|---|---|
| <kbd>Super</kbd> + <kbd>V</kbd> | Toggle clipboard popup at cursor / center |
| <kbd>↑</kbd> / <kbd>↓</kbd> | Navigate clipboard items |
| <kbd>Enter</kbd> | Copy selected item to clipboard and close window |
| <kbd>Ctrl</kbd> + <kbd>P</kbd> | Toggle pin on highlighted item |
| <kbd>Ctrl</kbd> + <kbd>D</kbd> | Delete highlighted item from history |
| <kbd>Escape</kbd> | Close popup without copying |

### CLI Commands
```bash
rclip toggle     # Toggle popup visibility (default)
rclip show       # Open popup
rclip hide       # Close popup
rclip clear      # Clear all unpinned history
rclip daemon     # Run in background daemon service mode
rclip add-clip "text" # Add text directly to history
```

### Configuration
`~/.config/rclip/config.json`:
```json
{
  "max_items": 100,
  "max_clip_bytes": 1000000
}
```

### Installation
```bash
./install.sh
```
This builds the release binary, installs it to `~/.local/bin/rclip`, installs the D-Bus service, copies the GNOME extension, and binds <kbd>Super</kbd>+<kbd>V</kbd>.

---

## appbox — User-Space AppImage & RPM Desktop Integrator

`appbox` converts downloaded `.AppImage` and `.rpm` packages into integrated user-space desktop apps (`.desktop` launcher, `hicolor` icons, `~/.local/bin/` CLI wrapper, and clean uninstall) without `sudo`, via CLI or GNOME Files right-click (**Scripts → Install as App**).

See [appbox/README.md](appbox/README.md) for usage and installation (`./appbox/install.sh`).
