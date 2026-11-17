# lanbeam

Local Wi-Fi transfer utility for Fedora Workstation — replaces sending files and links to yourself on WhatsApp. Share files, folders, photos, videos, and text between your computer and your phone over the local network, no cloud, no cables.

## Stack

| Component | Language | Description |
|---|---|---|
| Backend & CLI | Rust | Single native binary: LAN discovery, token auth, QR encoding (ISO/IEC 18004), streaming HTTP/1.1 server, on-the-fly ZIP packing, Wayland clipboard + notifications |
| Mobile web app | TypeScript | Phone-browser UI: file download list, "Download All (.zip)", multi-file upload with progress bars, two-way text/link sharing |
| Desktop popup | Python 3 + GTK4 | Native GNOME window: QR code, session URL, live counters, Open Downloads, Stop Sharing |

## Quick start

```bash
# From the lanbeam project directory
./install.sh

# Share a file or folder
lanbeam send ~/Pictures/Vacation

# Share with the GTK popup
lanbeam send ~/Pictures/Vacation --gui

# Receive uploads from your phone into ~/Downloads
lanbeam receive --dest ~/Downloads

# Beam text or a link
lanbeam text "https://example.com" --from-clipboard

# Generate a QR code for a URL
lanbeam qr "http://192.168.1.42:5000/abc123def456" --svg
```

On the phone, open the URL printed by `lanbeam send` (or scan the QR code). You'll see the shared files, can download them individually or as a `.zip`, and can upload photos/videos back to your computer.

## CLI

```
lanbeam send <path...> [--port PORT] [--timeout SEC] [--once] [--max-downloads N] [--gui] [--json]
lanbeam receive [--dest DIR] [--port PORT] [--timeout SEC] [--max-uploads N] [--gui] [--json]
lanbeam text [TEXT] [--from-clipboard] [--port PORT] [--timeout SEC] [--once] [--gui]
lanbeam qr <text_or_url> [--svg]
lanbeam setup-nautilus
```

## GNOME Files integration

`install.sh` registers `~/.local/share/nautilus/scripts/Share on LAN`. Right-click any file or folder in Nautilus → **Scripts → Share on LAN** to start sharing it over the LAN with the GTK popup.

## Build & test

From the repository root (`/home/img1/projects/Utilites`):

```bash
# 1. TypeScript frontend tests + bundle
cd lanbeam/web && bun test && bun run build.ts

# 2. Rust unit + integration tests
cargo test --manifest-path lanbeam/Cargo.toml --verbose

# 3. Release build
cargo build --release --manifest-path lanbeam/Cargo.toml

# 4. Python popup tests + syntax check
PYTHONPATH=lanbeam python3 -m unittest discover -s lanbeam/tests -v
python3 -m compileall -q lanbeam/ui lanbeam/tests

# 5. Shell lint (if shellcheck is installed)
shellcheck lanbeam/install.sh "lanbeam/nautilus/Share on LAN"
```

## Architecture

See `PLAN.md` for the detailed 8-step implementation plan, directory structure, and verification commands.

## License

MIT
