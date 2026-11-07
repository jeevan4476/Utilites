# `appbox` — User-Space AppImage & RPM Desktop Integrator

`appbox` turns downloaded `.AppImage` and `.rpm` packages into native-feeling user-space applications without `sudo`, following XDG FreeDesktop conventions.

## Features
- **AppImage integration:** Stores `.AppImage` binaries in `~/Applications/` (or unpacks with `--extract`), extracts `.desktop` + icons safely (`7z` with `--appimage-extract` fallback), and registers GNOME launcher entries.
- **Rootless RPM extraction:** Unpacks `.rpm` packages with `rpm2cpio` + `cpio` into `~/.local/share/appbox/apps/<id>/root/`, generates a `launch.sh` wrapper setting `LD_LIBRARY_PATH`, `PATH`, `XDG_DATA_DIRS`, and automatic Electron `--no-sandbox` fallback when `chrome-sandbox` lacks root SUID permissions.
- **Clean lifecycle:** Tracks every installed file in `~/.local/share/appbox/manifests/<id>.json` for deterministic upgrades and `appbox remove <id>`.
- **GNOME Files (Nautilus) integration:** Right-click any `.AppImage` or `.rpm` → **Scripts** → **Install as App**.

## Installation
```bash
./install.sh
```
Installs `~/.local/bin/appbox` and `~/.local/share/nautilus/scripts/Install as App`.

## CLI Usage
```bash
appbox install ~/Downloads/T3-Code-0.1.9-x86_64.AppImage
appbox install ~/Downloads/chatgpt.x86_64.rpm
appbox install ~/Downloads/app.AppImage --extract --move --name my-app
appbox list
appbox info t3-code
appbox remove t3-code
```

## Tests
```bash
PYTHONPATH=. python3 -m unittest discover -s tests -v
```
