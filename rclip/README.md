# rclip — Windows `Win + V` Style Clipboard Manager for GNOME Wayland

`rclip` is a native GNOME Shell (45–51) clipboard overlay + CLI companion that replicates Windows `Win + V` behavior on Fedora Workstation / GNOME Wayland with zero GTK4 window glitches and zero root permissions.

## Key Features
- **Instant `Super + V` Overlay:** Renders natively inside GNOME Shell right near your cursor, clamped to the current monitor work area, and dismisses automatically when you click outside or press `Esc`.
- **Text + Images with Thumbnails:** Captures text, URLs, code, and copied PNG images/screenshots with thumbnail previews, dimensions, and relative timestamps.
- **True Auto-Paste:** Selecting a clip (`Enter` or single click) restores focus to your previous window, updates the clipboard, and injects `Ctrl+V` (or `Ctrl+Shift+V` in terminals like Ptyxis, GNOME Terminal, Alacritty, Kitty, WezTerm, Foot, Konsole).
- **Pin, Search, & Clear All:** Real-time search box, `★` pinning (`Ctrl+P`), single-item delete (`Ctrl+D` / `Delete`), and a **`Clear all`** header button that preserves pinned items.
- **Privacy & Security:** Automatically ignores copies flagged with `x-kde-passwordManagerHint` (1Password, Bitwarden, KeePassXC) and stores data with `0o700` directory / `0o600` file permissions.

## Keyboard Controls
| Shortcut | Action |
|---|---|
| <kbd>Super</kbd> + <kbd>V</kbd> | Toggle clipboard popup at cursor |
| <kbd>↑</kbd> / <kbd>↓</kbd> | Navigate items (auto-scrolls list) |
| <kbd>Enter</kbd> or Click | Paste selected item into previous window |
| <kbd>Ctrl</kbd> + <kbd>P</kbd> | Pin / unpin selected item |
| <kbd>Ctrl</kbd> + <kbd>D</kbd> | Delete selected item |
| <kbd>Esc</kbd> or Click outside | Close popup |

## CLI Commands
```bash
rclip toggle   # Toggle popup via D-Bus
rclip show     # Show popup
rclip hide     # Hide popup
rclip clear    # Clear unpinned history (keeps pinned)
rclip list     # List saved text & image clips (--json supported)
rclip status   # Show history path, item count, and extension status
```

## Installation
```bash
./install.sh
```
Installs `rclip@utilities.local` into `~/.local/share/gnome-shell/extensions/`, compiles its GSettings schema for `<Super>v`, installs `~/.local/bin/rclip`, and cleans up any legacy `rclip-watcher` setup. On Wayland, log out and back in once so GNOME Shell loads the new extension.

## Tests
```bash
gjs -m tests/test_store.mjs
PYTHONPATH=. python3 -m unittest discover -s tests -v
```
