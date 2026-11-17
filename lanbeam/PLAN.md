# `lanbeam` implementation plan

`lanbeam` replaces sending files and links to yourself on WhatsApp with a local Wi-Fi transfer utility for Fedora Workstation. The core CLI, streaming HTTP server, on-the-fly ZIP packer, and QR encoder are written in Rust. The mobile web app is written in TypeScript, and the desktop QR popup window uses Python 3 with GTK4.

---

## 1. Architecture overview

- **Rust backend and CLI (`Cargo.toml`, `src/*.rs`):** Compiles into a single native `lanbeam` binary using Rust standard library networking and file I/O. Handles LAN IPv4 discovery, token generation, ISO/IEC 18004 QR code generation, multi-threaded HTTP/1.1 streaming for downloads and uploads, on-the-fly `.zip` streaming for folders, Wayland clipboard sync via `wl-copy` and `wl-paste`, and desktop notifications via `notify-send`.
- **TypeScript mobile web frontend (`web/*.ts`, `web/index.html`, `web/styles.css`):** Provides the phone browser interface for downloading shared files, uploading phone photos or videos to `~/Downloads` with live progress bars and transfer rates, and beaming text or links in both directions. Bundled into `src/web_assets.rs` and embedded directly into the Rust binary via `include_str!`.
- **Python 3 GTK4 desktop popup (`ui/popup.py`):** Renders a native GNOME GTK4 window via PyGObject when triggered from GNOME Files or with `--gui`, displaying the QR SVG, session URL, live transfer counters, an "Open Downloads" button, and a "Stop Sharing" button.

---

## 2. Directory structure and exact file paths

1. `/home/img1/projects/Utilites/lanbeam/PLAN.md`
2. `/home/img1/projects/Utilites/lanbeam/README.md`
3. `/home/img1/projects/Utilites/lanbeam/Cargo.toml`
4. `/home/img1/projects/Utilites/lanbeam/install.sh`
5. `/home/img1/projects/Utilites/lanbeam/nautilus/Share on LAN`
6. `/home/img1/projects/Utilites/lanbeam/src/main.rs`
7. `/home/img1/projects/Utilites/lanbeam/src/lib.rs`
8. `/home/img1/projects/Utilites/lanbeam/src/cli.rs`
9. `/home/img1/projects/Utilites/lanbeam/src/network.rs`
10. `/home/img1/projects/Utilites/lanbeam/src/qr.rs`
11. `/home/img1/projects/Utilites/lanbeam/src/zip.rs`
12. `/home/img1/projects/Utilites/lanbeam/src/server.rs`
13. `/home/img1/projects/Utilites/lanbeam/src/web_assets.rs`
14. `/home/img1/projects/Utilites/lanbeam/web/package.json`
15. `/home/img1/projects/Utilites/lanbeam/web/tsconfig.json`
16. `/home/img1/projects/Utilites/lanbeam/web/types.ts`
17. `/home/img1/projects/Utilites/lanbeam/web/app.ts`
18. `/home/img1/projects/Utilites/lanbeam/web/app.test.ts`
19. `/home/img1/projects/Utilites/lanbeam/web/styles.css`
20. `/home/img1/projects/Utilites/lanbeam/web/index.html`
21. `/home/img1/projects/Utilites/lanbeam/web/build.ts`
22. `/home/img1/projects/Utilites/lanbeam/ui/popup.py`
23. `/home/img1/projects/Utilites/lanbeam/tests/test_popup.py`
24. `/home/img1/projects/Utilites/README.md`
25. `/home/img1/projects/Utilites/.github/workflows/ci.yml`
26. `/home/img1/projects/Utilites/.github/workflows/release.yml`

---

## 3. Numbered implementation steps

### Step 1. Rust crate setup and CLI parser
- **File:** `/home/img1/projects/Utilites/lanbeam/Cargo.toml`
- **File:** `/home/img1/projects/Utilites/lanbeam/src/main.rs`
- **File:** `/home/img1/projects/Utilites/lanbeam/src/lib.rs`
- **File:** `/home/img1/projects/Utilites/lanbeam/src/cli.rs`
- Configure package `lanbeam` version `0.1.0` with edition `2021`.
- Implement CLI subcommands and flags in `src/cli.rs`:
  - `lanbeam send <path...> [--port PORT] [--timeout SEC] [--once] [--max-downloads N] [--gui] [--json]`
  - `lanbeam receive [--dest DIR] [--port PORT] [--timeout SEC] [--max-uploads N] [--gui] [--json]`
  - `lanbeam text [TEXT] [--from-clipboard] [--port PORT] [--timeout SEC] [--once] [--gui]`
  - `lanbeam qr <text_or_url> [--svg]`
  - `lanbeam setup-nautilus`

### Step 2. LAN IPv4 discovery, token generator, and Fedora port binding in Rust
- **File:** `/home/img1/projects/Utilites/lanbeam/src/network.rs`
- Implement `detect_lan_ip() -> IpAddr` using a UDP socket connect probe to `10.255.255.255:1` to discover the outbound Wi-Fi interface IP on Fedora (`wlp0s20f3`), falling back to `127.0.0.1` offline.
- Implement `bind_listener(host: IpAddr, preferred_port: u16) -> io::Result<(TcpListener, u16)>` binding in TCP range `1025..=65535`, which Fedora Workstation's default `FedoraWorkstation` firewalld zone permits without root privileges.
- Implement `generate_token() -> String` reading 9 random bytes from `/dev/urandom` and encoding them as a 12-character base62 token.

### Step 3. Pure-Rust QR code encoder
- **File:** `/home/img1/projects/Utilites/lanbeam/src/qr.rs`
- Implement an ISO/IEC 18004 QR Code encoder in Rust supporting byte mode across versions 1 through 6 with Reed-Solomon GF256 error correction level L and penalty mask evaluation:
  - `QrCode::encode(text: &str) -> Result<QrCode, QrError>`
  - `QrCode::to_ansi(&self) -> String` using Unicode half-blocks `▀`, `▄`, `█`, and space for terminal display.
  - `QrCode::to_svg(&self, scale: u32) -> String` for the Python GTK4 window and browser views.

### Step 4. On-the-fly ZIP archive packer in Rust
- **File:** `/home/img1/projects/Utilites/lanbeam/src/zip.rs`
- Implement a streaming ZIP writer with CRC32 calculation and standard local/central directory headers so sharing multiple files or a directory (`lanbeam send ~/Pictures/Trip`) lets the phone download everything as a single `.zip` without external archive binaries.

### Step 5. TypeScript mobile web frontend
- **File:** `/home/img1/projects/Utilites/lanbeam/web/package.json`
- **File:** `/home/img1/projects/Utilites/lanbeam/web/tsconfig.json`
- **File:** `/home/img1/projects/Utilites/lanbeam/web/types.ts`
- **File:** `/home/img1/projects/Utilites/lanbeam/web/app.ts`
- **File:** `/home/img1/projects/Utilites/lanbeam/web/app.test.ts`
- **File:** `/home/img1/projects/Utilites/lanbeam/web/styles.css`
- **File:** `/home/img1/projects/Utilites/lanbeam/web/index.html`
- **File:** `/home/img1/projects/Utilites/lanbeam/web/build.ts`
- **File:** `/home/img1/projects/Utilites/lanbeam/src/web_assets.rs`
- Build the mobile transfer application in TypeScript:
  - Typed API client for `GET /<token>/status`, `POST /<token>/upload?name=<name>`, and `POST /<token>/text`.
  - Pure helper functions in `web/app.ts` for byte size formatting, transfer speed (`MB/s`), ETA calculation, and filename extraction, unit-tested in `web/app.test.ts`.
  - Responsive layout with dark/light support, file download list, "Download All (.zip)" button, multi-file phone upload queue with `XMLHttpRequest.upload.onprogress` progress bars, and two-way clipboard/link sharing.
  - `web/build.ts` compiles `web/app.ts` and `web/styles.css` into a single inline HTML string written to `src/web_assets.rs` so `cargo build` embeds the complete frontend inside the `lanbeam` binary.

### Step 6. Rust multi-threaded streaming HTTP server
- **File:** `/home/img1/projects/Utilites/lanbeam/src/server.rs`
- Implement `ServerSession` over `std::net::TcpListener`:
  - Reject any path outside `/<token>` with `403 Forbidden`.
  - `GET /<token>`: serves the compiled TypeScript web app with initial session state JSON injected.
  - `GET /<token>/qr.svg`: serves the session SVG QR code.
  - `GET /<token>/status`: returns JSON state (`mode`, `url`, `files`, `downloads_completed`, `uploads_received`, `shared_text`, `remaining_seconds`, `active`).
  - `GET /<token>/download/<id>/<name>`: streams shared files in 64 KiB chunks with HTTP `Range` support for mobile video and large file resume; increments download count and triggers shutdown when `--once` or `--max-downloads` is met.
  - `GET /<token>/download-all.zip`: streams all shared files or directory contents as a `.zip` archive.
  - `POST /<token>/upload?name=<filename>`: streams raw request bytes in 64 KiB chunks directly to `<dest>/.<filename>.lanbeam.part`, sanitizes filenames, deduplicates names as `file_1.ext`, renames atomically upon completion, and triggers `notify-send`.
  - `POST /<token>/text`: accepts text or URLs from the phone, pipes to `wl-copy` on Wayland, records the snippet in session state, and sends a desktop notification.
  - `POST /<token>/stop`: cleanly terminates the listener loop when clicked in the GTK4 popup.

### Step 7. Python 3 GTK4 desktop QR popup and GNOME Files script
- **File:** `/home/img1/projects/Utilites/lanbeam/ui/popup.py`
- **File:** `/home/img1/projects/Utilites/lanbeam/tests/test_popup.py`
- **File:** `/home/img1/projects/Utilites/lanbeam/nautilus/Share on LAN`
- **File:** `/home/img1/projects/Utilites/lanbeam/install.sh`
- Build `ui/popup.py` using Python 3 `gi.repository.Gtk` (GTK 4.0) and `GdkPixbuf`:
  - Renders the QR SVG inside a `Gtk.Picture` on a white card with the session URL, "Copy Link" button, live transfer status label polling `/<token>/status` every 1000 ms, "Open Downloads" button, and "Stop Sharing" (`Esc`) button.
  - Includes a `--self-test` / `--help` CLI flag for headless verification in CI.
- Build `nautilus/Share on LAN` and `install.sh` to compile the Rust binary (`cargo build --release`), install `~/.local/bin/lanbeam`, and register `~/.local/share/nautilus/scripts/Share on LAN`.

### Step 8. Documentation and GitHub Actions CI/CD integration
- **File:** `/home/img1/projects/Utilites/lanbeam/README.md`
- **File:** `/home/img1/projects/Utilites/README.md`
- **File:** `/home/img1/projects/Utilites/.github/workflows/ci.yml`
- **File:** `/home/img1/projects/Utilites/.github/workflows/release.yml`
- Document CLI usage, GNOME Files workflow, and architecture in `lanbeam/README.md` and root `README.md`.
- Add `lanbeam` jobs to `ci.yml` and `release.yml` running Rust unit/integration tests, TypeScript tests, Python popup tests, and `shellcheck`.

---

## 4. Verification and test commands

Run these exact commands from `/home/img1/projects/Utilites` to verify every layer of the stack:

1. **Bundle TypeScript frontend and run TypeScript unit tests:**
   ```bash
   cd /home/img1/projects/Utilites/lanbeam/web && bun test && bun run build.ts
   ```

2. **Run Rust unit and HTTP integration tests (QR, ZIP, network, streaming upload/download, CLI):**
   ```bash
   cargo test --manifest-path /home/img1/projects/Utilites/lanbeam/Cargo.toml --verbose
   ```

3. **Build release Rust binary and verify CLI output:**
   ```bash
   cargo build --release --manifest-path /home/img1/projects/Utilites/lanbeam/Cargo.toml && /home/img1/projects/Utilites/lanbeam/target/release/lanbeam --version
   ```

4. **Run Python GTK4 popup unit tests and syntax checks:**
   ```bash
   PYTHONPATH=/home/img1/projects/Utilites/lanbeam python3 -m unittest discover -s /home/img1/projects/Utilites/lanbeam/tests -v && python3 -m compileall -q /home/img1/projects/Utilites/lanbeam/ui /home/img1/projects/Utilites/lanbeam/tests
   ```

5. **Lint installer and Nautilus scripts with `shellcheck`:**
   ```bash
   shellcheck /home/img1/projects/Utilites/lanbeam/install.sh "/home/img1/projects/Utilites/lanbeam/nautilus/Share on LAN"
   ```
