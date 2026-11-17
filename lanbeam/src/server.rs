//! Multi-threaded streaming HTTP server (Step 6).
//!
//! Implements `ServerSession` over `std::net::TcpListener` serving the lanbeam
//! transfer app on `GET /<token>` with file download, upload, text, and QR
//! endpoints. This is the skeleton; endpoint bodies are filled in below.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::qr::QrCode;
use crate::web_assets::WEB_ASSETS;
use crate::zip::{write_zip_files, ZipFileEntry};

/// Uploads are written in 64 KiB chunks.
const UPLOAD_CHUNK_SIZE: usize = 64 * 1024;
/// Maximum HTTP request header size (16 KiB).
const MAX_HEADER_BYTES: usize = 16 * 1024;
/// Maximum JSON body size accepted on `/text` (1 MiB).
const MAX_TEXT_BODY_BYTES: usize = 1024 * 1024;
/// Maximum filename length leaving room for `.` and `.lanbeam.part` inside 255 bytes.
const MAX_UPLOAD_FILENAME_BYTES: usize = 240;

static PART_SEQ: AtomicU64 = AtomicU64::new(0);

fn lock_state(state: &Arc<Mutex<SessionState>>) -> MutexGuard<'_, SessionState> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

fn spawn_detached(mut cmd: Command) {
    if let Ok(mut child) = cmd.spawn() {
        thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Shared session state.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub mode: String,
    pub url: String,
    pub files: Vec<String>,
    pub downloads_completed: u32,
    pub uploads_received: u32,
    pub shared_text: Option<String>,
    pub remaining_seconds: u64,
    pub active: bool,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            mode: "send".to_string(),
            url: String::new(),
            files: Vec::new(),
            downloads_completed: 0,
            uploads_received: 0,
            shared_text: None,
            remaining_seconds: 0,
            active: true,
        }
    }
}

/// An HTTP server session bound to a single token.
pub struct ServerSession {
    listener: TcpListener,
    token: String,
    state: Arc<Mutex<SessionState>>,
    port: u16,
    dest: PathBuf,
    /// Files/directories being shared (for send mode).
    share_paths: Vec<PathBuf>,
    /// Stop after first transfer completes (--once).
    once: bool,
    /// Maximum number of downloads before stopping (--max-downloads).
    max_downloads: Option<u32>,
    /// Maximum number of uploads before stopping (--max-uploads).
    max_uploads: Option<u32>,
    /// Inactivity timeout in seconds (--timeout).
    timeout: Option<u64>,
}

impl ServerSession {
    /// Create a new session bound to `listener` on `port` serving `token`.
    /// Uploads are stored in `~/Downloads` (or the current directory when the
    /// home directory is unavailable) unless overridden with `with_dest`.
    pub fn new(listener: TcpListener, port: u16, token: &str) -> io::Result<Self> {
        Ok(Self {
            listener,
            token: token.to_string(),
            state: Arc::new(Mutex::new(SessionState::default())),
            port,
            dest: default_upload_dir(),
            share_paths: Vec::new(),
            once: false,
            max_downloads: None,
            max_uploads: None,
            timeout: None,
        })
    }

    /// Set the session mode reported by `/status` (`"send"`, `"receive"`, `"text"`).
    pub fn with_mode(self, mode: &str) -> Self {
        lock_state(&self.state).mode = mode.to_string();
        self
    }

    /// Set the full session URL reported by `/status` and encoded in `/qr.svg`.
    pub fn with_url(self, url: &str) -> Self {
        lock_state(&self.state).url = url.to_string();
        self
    }

    /// Set initial shared text reported by `/status`.
    pub fn with_shared_text(self, text: Option<String>) -> Self {
        lock_state(&self.state).shared_text = text;
        self
    }

    /// Override the directory received uploads are stored in.
    pub fn with_dest(mut self, dest: PathBuf) -> Self {
        self.dest = dest;
        self
    }

    /// Set the files/directories being shared (for send mode).
    pub fn with_share_paths(mut self, paths: Vec<PathBuf>) -> Self {
        let file_names: Vec<String> = paths
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
            .collect();
        lock_state(&self.state).files = file_names;
        self.share_paths = paths;
        self
    }

    /// Enable --once: stop after first transfer completes.
    pub fn with_once(mut self, once: bool) -> Self {
        self.once = once;
        self
    }

    /// Set --max-downloads: stop after N downloads complete.
    pub fn with_max_downloads(mut self, max_downloads: Option<u32>) -> Self {
        self.max_downloads = max_downloads;
        self
    }

    /// Set --max-uploads: stop after N uploads complete.
    pub fn with_max_uploads(mut self, max_uploads: Option<u32>) -> Self {
        self.max_uploads = max_uploads;
        self
    }

    /// Set --timeout: shut down after N seconds of inactivity.
    pub fn with_timeout(mut self, timeout: Option<u64>) -> Self {
        if let Some(secs) = timeout {
            lock_state(&self.state).remaining_seconds = secs;
        }
        self.timeout = timeout;
        self
    }

    /// The TCP port the listener is bound to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The session token.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Run the listener loop, handling each connection on its own thread until
    /// `stop()`, inactivity timeout, or transfer limit deactivates the session,
    /// then join all in-flight connection threads before returning.
    pub fn run(self) -> io::Result<()> {
        self.listener.set_nonblocking(true)?;
        let state = self.state.clone();
        let token = self.token.clone();
        let dest = self.dest.clone();
        let share_paths = self.share_paths.clone();
        let once = self.once;
        let max_downloads = self.max_downloads;
        let max_uploads = self.max_uploads;
        let timeout = self.timeout;
        let last_activity = Arc::new(Mutex::new(Instant::now()));
        let mut handles: Vec<thread::JoinHandle<()>> = Vec::new();

        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    *last_activity.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
                    handles.retain(|h| !h.is_finished());
                    let state = state.clone();
                    let token = token.clone();
                    let dest = dest.clone();
                    let share_paths = share_paths.clone();
                    let last_activity = last_activity.clone();
                    handles.push(thread::spawn(move || {
                        if let Err(e) = handle_connection(
                            stream,
                            &token,
                            &state,
                            &dest,
                            &share_paths,
                            once,
                            max_downloads,
                            max_uploads,
                            &last_activity,
                        ) {
                            eprintln!("lanbeam: connection error: {e}");
                        }
                    }));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    let active = if let Some(secs) = timeout {
                        let elapsed = last_activity
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .elapsed()
                            .as_secs();
                        let rem = secs.saturating_sub(elapsed);
                        let mut st = lock_state(&self.state);
                        st.remaining_seconds = rem;
                        if elapsed >= secs {
                            st.active = false;
                        }
                        st.active
                    } else {
                        lock_state(&self.state).active
                    };
                    if !active {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Err(e);
                }
            }
        }

        for handle in handles {
            let _ = handle.join();
        }
        Ok(())
    }

    /// Cleanly terminate the listener loop.
    pub fn stop(&self) {
        lock_state(&self.state).active = false;
    }
}

/// Read HTTP headers up to `\r\n\r\n` (capped at `MAX_HEADER_BYTES`), returning
/// the raw header bytes and any trailing body bytes already read.
fn read_http_head(stream: &mut TcpStream) -> io::Result<Option<(Vec<u8>, Vec<u8>)>> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let header_end = pos + 4;
            let body = buf[header_end..].to_vec();
            buf.truncate(header_end);
            return Ok(Some((buf, body)));
        }
        if buf.len() >= MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request headers exceed limit",
            ));
        }
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete request headers",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

/// Handle a single HTTP connection.
#[allow(clippy::too_many_arguments)]
fn handle_connection(
    mut stream: TcpStream,
    token: &str,
    state: &Arc<Mutex<SessionState>>,
    dest: &Path,
    share_paths: &[PathBuf],
    once: bool,
    max_downloads: Option<u32>,
    max_uploads: Option<u32>,
    last_activity: &Arc<Mutex<Instant>>,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let (head_bytes, body_bytes) = match read_http_head(&mut stream) {
        Ok(Some(parts)) => parts,
        Ok(None) => return Ok(()),
        Err(_) => return respond(&mut stream, 400, "text/plain", b"Bad Request"),
    };

    let req = String::from_utf8_lossy(&head_bytes);
    let content_length = req
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("content-length:"))
        .and_then(|line| line.split_once(':'))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok());

    let mut lines = req.lines();
    let request_line = lines.next().unwrap_or("");
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return respond(&mut stream, 400, "text/plain", b"Bad Request");
    }
    let method = parts[0];
    let path = parts[1];
    let (route, _query) = path.split_once('?').unwrap_or((path, ""));

    // Reject any path outside /<token> (strict boundary check, including /).
    let expected_prefix = format!("/{}", token);
    let token_matches = route == expected_prefix
        || route
            .strip_prefix(&expected_prefix)
            .is_some_and(|rest| rest.starts_with('/'));
    if !token_matches {
        return respond(&mut stream, 403, "text/plain", b"Forbidden");
    }

    *last_activity.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();

    let range_header = req
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("range:"))
        .and_then(|line| line.split_once(':'))
        .map(|(_, v)| v.trim().to_string());

    match (method, route) {
        ("GET", r) if r == expected_prefix || r == format!("{expected_prefix}/") => {
            serve_app(&mut stream, state)
        }
        ("GET", r) if r == format!("{expected_prefix}/qr.svg") => {
            serve_qr(&mut stream, token, state)
        }
        ("GET", r) if r == format!("{expected_prefix}/status") => serve_status(&mut stream, state),
        ("GET", r) if r.starts_with(&format!("{expected_prefix}/download/")) => {
            serve_download(&mut stream, r, state, share_paths, once, max_downloads, range_header)
        }
        ("GET", r) if r == format!("{expected_prefix}/download-all.zip") => {
            serve_download_all(&mut stream, share_paths)
        }
        ("POST", r) if r == format!("{expected_prefix}/upload") => serve_upload(
            &mut stream,
            path,
            state,
            dest,
            &body_bytes,
            content_length,
            max_uploads,
        ),
        ("POST", r) if r == format!("{expected_prefix}/text") => {
            let full_body = read_bounded_body(&mut stream, &body_bytes, content_length, MAX_TEXT_BODY_BYTES)?;
            let body = String::from_utf8_lossy(&full_body);
            serve_text(&mut stream, state, &body, once)
        }
        ("POST", r) if r == format!("{expected_prefix}/stop") => {
            lock_state(state).active = false;
            respond(&mut stream, 200, "text/plain", b"Stopping")
        }
        _ => respond(&mut stream, 404, "text/plain", b"Not Found"),
    }
}

fn read_bounded_body(
    stream: &mut TcpStream,
    initial: &[u8],
    content_length: Option<usize>,
    cap: usize,
) -> io::Result<Vec<u8>> {
    let target = content_length.unwrap_or(initial.len()).min(cap);
    let mut out = Vec::with_capacity(target);
    let take = initial.len().min(target);
    out.extend_from_slice(&initial[..take]);
    let mut tmp = [0u8; 4096];
    while out.len() < target {
        let need = (target - out.len()).min(tmp.len());
        let n = stream.read(&mut tmp[..need])?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&tmp[..n]);
    }
    Ok(out)
}

/// Default upload destination: `~/Downloads`, falling back to the cwd.
fn default_upload_dir() -> PathBuf {
    std::env::home_dir()
        .map(|home| home.join("Downloads"))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> io::Result<()> {
    let status_text = match status {
        200 => "OK",
        206 => "Partial Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        411 => "Length Required",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        _ => "Unknown",
    };
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status, status_text, content_type, body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.write_all(body)?;
    Ok(())
}

fn serve_app(stream: &mut TcpStream, _state: &Arc<Mutex<SessionState>>) -> io::Result<()> {
    // Serve the embedded TypeScript web app. The frontend polls /status on load
    // to pick up the initial session state.
    respond(stream, 200, "text/html; charset=utf-8", WEB_ASSETS.as_bytes())
}

fn serve_qr(stream: &mut TcpStream, token: &str, state: &Arc<Mutex<SessionState>>) -> io::Result<()> {
    let url = lock_state(state).url.clone();
    let payload = if url.is_empty() { token } else { url.as_str() };
    match QrCode::encode(payload) {
        Ok(qr) => {
            let svg = qr.to_svg(4);
            respond(stream, 200, "image/svg+xml", svg.as_bytes())
        }
        Err(_) => respond(stream, 500, "text/plain", b"QR encode failed"),
    }
}

fn serve_status(stream: &mut TcpStream, state: &Arc<Mutex<SessionState>>) -> io::Result<()> {
    let st = lock_state(state);
    let files_json = st
        .files
        .iter()
        .map(|f| format!("\"{}\"", escape_json(f)))
        .collect::<Vec<_>>()
        .join(",");
    let shared_text_json = match &st.shared_text {
        Some(t) => format!("\"shared_text\":\"{}\"", escape_json(t)),
        None => "\"shared_text\":null".to_string(),
    };
    let body = format!(
        "{{\"mode\":\"{}\",\"url\":\"{}\",\"files\":[{}],\"downloads_completed\":{},\"uploads_received\":{},{},\"remaining_seconds\":{},\"active\":{}}}",
        escape_json(&st.mode),
        escape_json(&st.url),
        files_json,
        st.downloads_completed,
        st.uploads_received,
        shared_text_json,
        st.remaining_seconds,
        st.active
    );
    respond(stream, 200, "application/json", body.as_bytes())
}

/// Escape special and control characters for JSON string values.
fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Download chunk size: 64 KiB.
const DOWNLOAD_CHUNK_SIZE: usize = 64 * 1024;

/// Stream a single file for download with HTTP Range support.
///
/// Path format: `/<token>/download/<index>/<name>` where `<index>` is the
/// index into `share_paths`.
fn serve_download(
    stream: &mut TcpStream,
    path: &str,
    state: &Arc<Mutex<SessionState>>,
    share_paths: &[PathBuf],
    once: bool,
    max_downloads: Option<u32>,
    range_header: Option<String>,
) -> io::Result<()> {
    // Parse /<token>/download/<index>/<name>
    let prefix = "/download/";
    let Some((_before, rest)) = path.split_once(prefix) else {
        return respond(stream, 404, "text/plain", b"Not Found");
    };
    let (idx_str, _name) = match rest.split_once('/') {
        Some((i, n)) => (i, n),
        None => return respond(stream, 404, "text/plain", b"Not Found"),
    };
    let idx: usize = match idx_str.parse() {
        Ok(i) => i,
        Err(_) => return respond(stream, 404, "text/plain", b"Not Found"),
    };

    // Get the file path to serve.
    let file_path = match share_paths.get(idx) {
        Some(p) => p,
        None => return respond(stream, 404, "text/plain", b"Not Found"),
    };

    // Verify metadata without following symlinks and reject directories before sending headers.
    let meta = match fs::symlink_metadata(file_path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return respond(stream, 404, "text/plain", b"Not Found");
        }
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink() {
        return respond(stream, 403, "text/plain", b"Forbidden");
    }
    if !meta.is_file() {
        return respond(stream, 404, "text/plain", b"Not Found");
    }

    let mut file = match fs::File::open(file_path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return respond(stream, 404, "text/plain", b"Not Found");
        }
        Err(e) => return Err(e),
    };

    let file_len = file.metadata()?.len();
    if file_len == 0 {
        if range_header.is_some() {
            let resp = "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            stream.write_all(resp.as_bytes())?;
            return Ok(());
        }
        let content_type = content_type_from_path(file_path);
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: 0\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
            content_type
        );
        stream.write_all(header.as_bytes())?;
        record_download_completion(state, once, max_downloads);
        return Ok(());
    }

    // Parse Range header if present.
    let range = parse_range_header(range_header, file_len);
    let (start, raw_end) = match range {
        Some((s, e)) => (s, e),
        None => (0, file_len - 1),
    };

    if start > raw_end || start >= file_len {
        let resp = format!(
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            file_len
        );
        stream.write_all(resp.as_bytes())?;
        return Ok(());
    }
    let end = raw_end.min(file_len - 1);

    let content_length = end - start + 1;
    let status = if range.is_some() { 206 } else { 200 };
    let status_text = if status == 206 { "Partial Content" } else { "OK" };
    let content_type = content_type_from_path(file_path);

    let mut header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n",
        status, status_text, content_type, content_length
    );
    if range.is_some() {
        header.push_str(&format!("Content-Range: bytes {}-{}/{}\r\n", start, end, file_len));
    }
    header.push_str("\r\n");
    stream.write_all(header.as_bytes())?;

    file.seek(std::io::SeekFrom::Start(start))?;

    // Stream in 64 KiB chunks.
    let mut remaining = content_length;
    let mut chunk = vec![0u8; DOWNLOAD_CHUNK_SIZE.min(content_length as usize)];
    while remaining > 0 {
        let to_read = chunk.len().min(remaining as usize);
        let n = file.read(&mut chunk[..to_read])?;
        if n == 0 {
            break;
        }
        stream.write_all(&chunk[..n])?;
        remaining -= n as u64;
    }

    // Only record a completed download if all requested bytes were transferred and
    // either the request was un-ranged or reached the final byte of the file.
    if remaining == 0 && (range.is_none() || end + 1 == file_len) {
        record_download_completion(state, once, max_downloads);
    }

    Ok(())
}

fn record_download_completion(
    state: &Arc<Mutex<SessionState>>,
    once: bool,
    max_downloads: Option<u32>,
) {
    let mut st = lock_state(state);
    st.downloads_completed += 1;
    if once || max_downloads.is_some_and(|max| st.downloads_completed >= max) {
        st.active = false;
    }
}

/// Parse the Range header value (e.g., "bytes=0-1023") into (start, end).
/// Returns None if header is missing, malformed, or unsatisfiable.
/// End is inclusive.
fn parse_range_header(range_header: Option<String>, file_len: u64) -> Option<(u64, u64)> {
    let range_header = range_header?;
    if file_len == 0 {
        return None;
    }
    let range_header = range_header.strip_prefix("bytes=")?;
    let (start_str, end_str) = range_header.split_once('-')?;

    if start_str.is_empty() && end_str.is_empty() {
        return None;
    }

    let start = if start_str.is_empty() {
        // Suffix-byte-range: bytes=-500 means last 500 bytes
        let suffix = end_str.parse::<u64>().ok()?;
        if suffix == 0 {
            return None;
        }
        if suffix >= file_len {
            0
        } else {
            file_len - suffix
        }
    } else {
        start_str.parse::<u64>().ok()?
    };

    let end = if end_str.is_empty() || start_str.is_empty() {
        file_len - 1
    } else {
        end_str.parse::<u64>().ok()?
    };

    Some((start, end))
}

/// Guess content type from file extension (simple built-in mapping).
fn content_type_from_path(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") | Some("htm") => "text/html",
        Some("txt") => "text/plain",
        Some("css") => "text/css",
        Some("js") | Some("mjs") => "application/javascript",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("tar") => "application/x-tar",
        Some("gz") => "application/gzip",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Stream all shared files as a ZIP archive without reading files into memory.
fn serve_download_all(
    stream: &mut TcpStream,
    share_paths: &[PathBuf],
) -> io::Result<()> {
    let entries = collect_zip_entries(share_paths)?;

    if entries.is_empty() {
        return respond(stream, 404, "text/plain", b"No files to download");
    }

    let header = "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; filename=\"download-all.zip\"\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
    stream.write_all(header.as_bytes())?;

    let mut writer = ChunkedWriter::new(stream);
    write_zip_files(&mut writer, &entries)?;
    writer.finish()?;

    Ok(())
}

/// Collect regular files from `share_paths` without following symlinks.
fn collect_zip_entries(share_paths: &[PathBuf]) -> io::Result<Vec<ZipFileEntry>> {
    let mut entries = Vec::new();
    for path in share_paths {
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_file() {
            if let Some(name) = path.file_name().map(|n| n.to_string_lossy().replace('\\', "/")) {
                entries.push(ZipFileEntry {
                    name,
                    path: path.clone(),
                });
            }
        } else if meta.is_dir() {
            let root_canon = fs::canonicalize(path)?;
            collect_dir_entries(&root_canon, &root_canon, &mut entries)?;
        }
    }
    Ok(entries)
}

/// Recursively collect regular files from a directory, skipping symlinks and
/// enforcing that every entry stays under `root_canon`.
fn collect_dir_entries(
    root_canon: &Path,
    current: &Path,
    entries: &mut Vec<ZipFileEntry>,
) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let canon = fs::canonicalize(&path)?;
        if !canon.starts_with(root_canon) {
            continue;
        }
        if meta.is_file() {
            let rel_path = canon.strip_prefix(root_canon).map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("strip_prefix failed: {e}"))
            })?;
            let name = rel_path.to_string_lossy().replace('\\', "/");
            entries.push(ZipFileEntry { name, path: canon });
        } else if meta.is_dir() {
            collect_dir_entries(root_canon, &canon, entries)?;
        }
    }
    Ok(())
}

/// Writer that outputs chunked transfer encoding.
struct ChunkedWriter<W: Write> {
    inner: W,
}

impl<W: Write> ChunkedWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner }
    }

    fn finish(&mut self) -> io::Result<()> {
        // Write final chunk (0 length).
        self.inner.write_all(b"0\r\n\r\n")
    }
}

impl<W: Write> Write for ChunkedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        // Write chunk size in hex.
        let size_hex = format!("{:x}\r\n", buf.len());
        self.inner.write_all(size_hex.as_bytes())?;
        self.inner.write_all(buf)?;
        self.inner.write_all(b"\r\n")?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Stream a `POST /<token>/upload?name=<filename>` request body to disk.
///
/// Requires `Content-Length`, streams bytes in `UPLOAD_CHUNK_SIZE` (64 KiB)
/// chunks into an exclusive `.<name>.lanbeam.part` file in `dest`, cleans up on
/// incomplete or errored transfers, and commits atomically without overwriting
/// existing files (`file_1.ext`, ...).
fn serve_upload(
    stream: &mut TcpStream,
    path: &str,
    state: &Arc<Mutex<SessionState>>,
    dest: &Path,
    body: &[u8],
    content_length: Option<usize>,
    max_uploads: Option<u32>,
) -> io::Result<()> {
    let Some(total) = content_length else {
        return respond(stream, 411, "text/plain", b"Length Required");
    };

    let raw_name = extract_query_param(path, "name").unwrap_or_else(|| "upload".to_string());
    let decoded_name = percent_decode(&raw_name);
    let safe_name = cap_filename_bytes(&sanitize_filename(&decoded_name), MAX_UPLOAD_FILENAME_BYTES);

    fs::create_dir_all(dest)?;

    let (part_path, mut file) = create_unique_part_file(dest, &safe_name)?;

    let write_res = stream_upload_bytes(stream, &mut file, body, total);
    let written = match write_res {
        Ok(n) => n,
        Err(e) => {
            drop(file);
            let _ = fs::remove_file(&part_path);
            return Err(e);
        }
    };

    if let Err(e) = file.flush() {
        drop(file);
        let _ = fs::remove_file(&part_path);
        return Err(e);
    }
    drop(file);

    if written != total {
        let _ = fs::remove_file(&part_path);
        let _ = respond(stream, 400, "text/plain", b"Incomplete Upload");
        return Ok(());
    }

    let final_name = match commit_part_unique(dest, &safe_name, &part_path) {
        Ok(name) => name,
        Err(e) => {
            let _ = fs::remove_file(&part_path);
            return Err(e);
        }
    };

    {
        let mut st = lock_state(state);
        st.uploads_received += 1;
        if max_uploads.is_some_and(|max| st.uploads_received >= max) {
            st.active = false;
        }
    }

    let mut cmd = Command::new("notify-send");
    cmd.args(["lanbeam", &format!("Received {}", final_name)]);
    spawn_detached(cmd);

    respond(stream, 200, "application/json", b"{\"ok\":true}")
}

fn stream_upload_bytes(
    stream: &mut TcpStream,
    file: &mut fs::File,
    initial_body: &[u8],
    total: usize,
) -> io::Result<usize> {
    let mut written = 0usize;
    if !initial_body.is_empty() {
        let take = initial_body.len().min(total);
        file.write_all(&initial_body[..take])?;
        written += take;
    }

    let mut chunk = [0u8; UPLOAD_CHUNK_SIZE];
    while written < total {
        let need = (total - written).min(chunk.len());
        match stream.read(&mut chunk[..need])? {
            0 => break,
            n => {
                file.write_all(&chunk[..n])?;
                written += n;
            }
        }
    }
    Ok(written)
}

fn create_unique_part_file(dest: &Path, safe_name: &str) -> io::Result<(PathBuf, fs::File)> {
    let primary = dest.join(format!(".{}.lanbeam.part", safe_name));
    match OpenOptions::new().write(true).create_new(true).open(&primary) {
        Ok(f) => return Ok((primary, f)),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }

    loop {
        let seq = PART_SEQ.fetch_add(1, Ordering::Relaxed);
        let alt_base = cap_filename_bytes(safe_name, MAX_UPLOAD_FILENAME_BYTES.saturating_sub(24));
        let candidate = dest.join(format!(".{}.{}.lanbeam.part", alt_base, seq));
        match OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(f) => return Ok((candidate, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

fn commit_part_unique(dest: &Path, safe_name: &str, part_path: &Path) -> io::Result<String> {
    for counter in 0u32.. {
        let candidate = if counter == 0 {
            safe_name.to_string()
        } else {
            increment_name(safe_name, counter)
        };
        let final_path = dest.join(&candidate);
        match fs::hard_link(part_path, &final_path) {
            Ok(()) => {
                let _ = fs::remove_file(part_path);
                return Ok(candidate);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                if !final_path.exists() {
                    fs::rename(part_path, &final_path)?;
                    return Ok(candidate);
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "exhausted unique filenames",
    ))
}

/// Extract a query parameter value from a request path, e.g. `?name=file.txt`.
fn extract_query_param(path: &str, key: &str) -> Option<String> {
    let (_path, query) = path.split_once('?')?;
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        if k == key {
            Some(v.to_string())
        } else {
            None
        }
    })
}

/// Percent-decode a URL query value (`+` as space, `%XX` as raw byte).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Truncate a sanitized filename (keeping its final extension if any) to fit `max_bytes`.
fn cap_filename_bytes(name: &str, max_bytes: usize) -> String {
    if name.len() <= max_bytes {
        return name.to_string();
    }
    if let Some(dot) = name.rfind('.') {
        let ext = &name[dot..];
        if dot > 0 && ext.len() < max_bytes {
            let keep = max_bytes - ext.len();
            let prefix = truncate_utf8(&name[..dot], keep);
            if !prefix.is_empty() {
                return format!("{prefix}{ext}");
            }
        }
    }
    let truncated = truncate_utf8(name, max_bytes);
    if truncated.is_empty() {
        "upload".to_string()
    } else {
        truncated.to_string()
    }
}

fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Strip directory components and unsafe characters from a filename.
/// Empty or `.`/`..` names fall back to `"upload"`.
fn sanitize_filename(name: &str) -> String {
    let replaced = name.replace('\\', "/");
    let basename = replaced.split('/').last().unwrap_or("").trim();

    let cleaned: String = basename
        .chars()
        .map(|c| {
            if c.is_ascii_control() || c == '\u{7f}' {
                '_'
            } else {
                c
            }
        })
        .collect();

    let cleaned = cleaned.trim_matches(|c| c == '.' || c == ' ');

    if cleaned.is_empty() || cleaned == ".." {
        "upload".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Return `name` if free in `dest`, otherwise `base_N.ext`, `base_(N+1).ext`, ...
/// The number is inserted before the final extension.
#[allow(dead_code)]
fn unique_name(dest: &Path, name: &str) -> String {
    let mut candidate = name.to_string();
    let mut counter = 1u32;
    while dest.join(&candidate).exists() {
        candidate = increment_name(name, counter);
        counter += 1;
    }
    candidate
}

/// Insert `_N` before the final extension of `name` (or append it).
fn increment_name(name: &str, n: u32) -> String {
    if let Some(dot) = name.rfind('.') {
        if dot > 0 {
            let (base, ext) = name.split_at(dot);
            return format!("{}_{}{}", base, n, ext);
        }
    }
    format!("{}_{}", name, n)
}

fn serve_text(
    stream: &mut TcpStream,
    state: &Arc<Mutex<SessionState>>,
    body: &str,
    once: bool,
) -> io::Result<()> {
    // Parse {"text": "..."} minimally without serde.
    let text = body
        .find("\"text\"")
        .and_then(|text_key_pos| {
            let after_key = &body[text_key_pos..];
            after_key.find(':').map(|colon_pos| (text_key_pos, colon_pos))
        })
        .and_then(|(text_key_pos, colon_pos)| {
            let s = &body[text_key_pos + colon_pos + 1..];
            let s = s.trim_start();
            let s = s.strip_prefix('"').unwrap_or(s);
            let end = s.find('"').unwrap_or(s.len());
            Some(s[..end].to_string())
        })
        .unwrap_or_default();

    if !text.is_empty() {
        {
            let mut st = lock_state(state);
            st.shared_text = Some(text.clone());
            if once {
                st.active = false;
            }
        }

        // Pipe to Wayland clipboard and notify the desktop without leaving zombies.
        let mut wl = Command::new("wl-copy");
        wl.arg(&text);
        spawn_detached(wl);

        let mut ns = Command::new("notify-send");
        ns.args(["lanbeam", &text]);
        spawn_detached(ns);
    }

    respond(stream, 200, "application/json", b"{\"ok\":true}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;
    use std::net::SocketAddr;

    #[test]
    fn session_state_defaults() {
        let st = SessionState::default();
        assert!(st.active);
        assert_eq!(st.downloads_completed, 0);
    }

    // --- filename sanitization ---

    #[test]
    fn sanitize_strips_directory_components() {
        assert_eq!(sanitize_filename("../../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("folder/photo.jpg"), "photo.jpg");
        assert_eq!(sanitize_filename("a\\b\\c.txt"), "c.txt");
        assert_eq!(sanitize_filename("./.config/secret"), "secret");
    }

    #[test]
    fn sanitize_rejects_empty_dot_and_spaces() {
        assert_eq!(sanitize_filename(""), "upload");
        assert_eq!(sanitize_filename("."), "upload");
        assert_eq!(sanitize_filename(".."), "upload");
        assert_eq!(sanitize_filename("   "), "upload");
    }

    #[test]
    fn sanitize_replaces_control_characters() {
        assert_eq!(sanitize_filename("a\u{0}b\u{7f}c"), "a_b_c");
        assert_eq!(sanitize_filename("name\n\r\t.txt"), "name___.txt");
    }

    // --- deduplication ---

    fn test_dest_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lanbeam-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create test upload dir");
        dir
    }

    #[test]
    fn unique_name_uses_name_when_free() {
        let dir = test_dest_dir();
        assert_eq!(unique_name(&dir, "file.txt"), "file.txt");
        assert_eq!(unique_name(&dir, "README"), "README");
    }

    #[test]
    fn unique_name_numbers_before_extension() {
        let dir = test_dest_dir();
        fs::File::create(dir.join("file.txt")).unwrap();
        fs::File::create(dir.join("file_1.txt")).unwrap();
        assert_eq!(unique_name(&dir, "file.txt"), "file_2.txt");
    }

    #[test]
    fn unique_name_numbers_names_without_extension() {
        let dir = test_dest_dir();
        fs::File::create(dir.join("README")).unwrap();
        assert_eq!(unique_name(&dir, "README"), "README_1");
        fs::File::create(dir.join("README_1")).unwrap();
        assert_eq!(unique_name(&dir, "README"), "README_2");
    }

    // --- upload over real HTTP ---

    /// Accept one connection and consume its request headers (mirroring
    /// `handle_connection`), returning the stream plus any body bytes that
    /// arrived with the headers.
    fn accept_upload(listener: &TcpListener) -> (TcpStream, Vec<u8>) {
        let (mut stream, _) = listener.accept().expect("accept");
        stream.set_nonblocking(false).expect("blocking stream");
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            let n = stream.read(&mut tmp).expect("read request headers");
            assert!(n > 0, "client disconnected before sending headers");
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let start = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| i + 4)
            .unwrap_or(buf.len());
        (stream, buf[start..].to_vec())
    }

    fn upload_client(addr: SocketAddr, token: &str, name: &str, body: &[u8]) -> Vec<u8> {
        let mut stream = TcpStream::connect(addr).expect("connect upload client");
        let request = format!(
            "POST /{}/upload?name={} HTTP/1.1\r\n\
             host: localhost\r\n\
             content-length: {}\r\n\
             connection: close\r\n\
             \r\n",
            token,
            name,
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("write request");
        stream.write_all(body).expect("write body");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read response");
        response
    }

    #[test]
    fn upload_streams_in_64kib_chunks_and_renames_atomically() {
        let dir = test_dest_dir();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "uptest";
        let state = Arc::new(Mutex::new(SessionState::default()));

        // Larger than one 64 KiB chunk to exercise the chunked read loop.
        let body: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
        let content_length = Some(body.len());
        let expected = body.clone();

        let client = std::thread::spawn(move || upload_client(addr, token, "video.bin", &body));

        let (mut stream, body_bytes) = accept_upload(&listener);
        let result = serve_upload(
            &mut stream,
            &format!("/{}/upload?name=video.bin", token),
            &state,
            &dir,
            &body_bytes,
            content_length,
            None,
        );
        assert!(result.is_ok(), "serve_upload should succeed");
        drop(stream); // EOF for the client, so read_to_end returns.

        let response = client.join().expect("client joined");
        assert!(response.starts_with(b"HTTP/1.1 200"), "unexpected: {:?}", String::from_utf8_lossy(&response));
        assert!(
            String::from_utf8_lossy(&response).contains("{\"ok\":true}"),
            "unexpected: {:?}",
            String::from_utf8_lossy(&response)
        );

        let final_path = dir.join("video.bin");
        let part_path = dir.join(".video.bin.lanbeam.part");
        assert!(final_path.is_file(), "final file should exist after atomic rename");
        assert!(!part_path.exists(), "part file should be gone after rename");
        assert_eq!(fs::read(&final_path).unwrap(), expected, "uploaded bytes must match");
        assert_eq!(state.lock().unwrap().uploads_received, 1);
    }

    #[test]
    fn upload_deduplicates_repeated_names() {
        let dir = test_dest_dir();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "uptest2";
        let state = Arc::new(Mutex::new(SessionState::default()));

        for expected in ["note.txt", "note_1.txt", "note_2.txt"] {
            let client = std::thread::spawn({
                let addr = addr;
                let token = token.to_string();
                move || upload_client(addr, &token, "note.txt", b"hello")
            });

            let (mut stream, body_bytes) = accept_upload(&listener);
            serve_upload(
                &mut stream,
                &format!("/{}/upload?name=note.txt", token),
                &state,
                &dir,
                &body_bytes,
                Some(5),
                None,
            )
            .expect("serve upload");
            drop(stream); // EOF for the client.

            client.join().expect("client joined");
            assert!(dir.join(expected).is_file(), "expected {}", expected);
        }
        assert_eq!(state.lock().unwrap().uploads_received, 3);
    }

    #[test]
    fn upload_sanitizes_filename_from_query() {
        let dir = test_dest_dir();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "uptest3";
        let state = Arc::new(Mutex::new(SessionState::default()));

        let client = std::thread::spawn(move || {
            upload_client(addr, token, "../../../evil.sh", b"nope")
        });

        let (mut stream, body_bytes) = accept_upload(&listener);
        serve_upload(
            &mut stream,
            &format!("/{}/upload?name=../../../evil.sh", token),
            &state,
            &dir,
            &body_bytes,
            Some(4),
            None,
        )
        .expect("serve upload");
        drop(stream); // EOF for the client.

        client.join().expect("client joined");
        assert!(dir.join("evil.sh").is_file(), "sanitized basename should be used");
        assert!(!dir.join("evil.sh.lanbeam.part").exists(), "no stray .part file");
        assert!(!dir.join("..").join("evil.sh").exists(), "no path traversal");
    }

    // --- stop ---

    #[test]
    fn stop_terminates_listener_loop() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("local addr").port();
        let session = ServerSession::new(listener, port, "tok").expect("new session");
        session.stop();
        // `active` is false, so `run` should exit on the first WouldBlock poll.
        assert!(session.run().is_ok(), "run should return Ok after stop");
    }

    #[test]
    fn stop_endpoint_response() {
        // `POST /<token>/stop` is handled inline in `handle_connection`: it
        // deactivates the session and replies 200. Exercise the state flip
        // directly through `ServerSession::stop`.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("local addr").port();
        let session = ServerSession::new(listener, port, "tok").expect("new session");
        assert!(session.state.lock().unwrap().active);
        session.stop();
        assert!(!session.state.lock().unwrap().active);
    }

    // --- download ---

    /// Helper to create a test file and return its path and content.
    fn create_test_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).expect("write test file");
        path
    }

    /// Accept one connection and consume its request headers (mirroring
    /// `handle_connection`), returning the stream.
    fn accept_download(listener: &TcpListener) -> TcpStream {
        let (mut stream, _) = listener.accept().expect("accept");
        stream.set_nonblocking(false).expect("blocking stream");
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            let n = stream.read(&mut tmp).expect("read request headers");
            assert!(n > 0, "client disconnected before sending headers");
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        stream
    }

    /// Helper to perform a download request and return response.
    fn download_request(
        addr: SocketAddr,
        token: &str,
        index: usize,
        range: Option<&str>,
    ) -> (u16, Vec<u8>, Vec<u8>) {
        let mut stream = TcpStream::connect(addr).expect("connect download client");
        let range_header = range.map(|r| format!("range: {}\r\n", r)).unwrap_or_default();
        let mut request = String::new();
        request.push_str(&format!("GET /{}/download/{}/test.txt HTTP/1.1\r\n", token, index));
        request.push_str("host: localhost\r\n");
        request.push_str(&range_header);
        request.push_str("connection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).expect("write request");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read response");
        
        // Parse status line.
        let status_line_end = response.windows(2).position(|w| w == b"\r\n").unwrap_or(0);
        let status_line = String::from_utf8_lossy(&response[..status_line_end]);
        let status_code = status_line.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
        
        // Find body start.
        let body_start = response.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(response.len());
        let headers = &response[..body_start];
        let body = &response[body_start..];
        
        (status_code, headers.to_vec(), body.to_vec())
    }

    /// Decode HTTP chunked transfer encoding.
    fn decode_chunked(data: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        let mut pos = 0;
        while pos < data.len() {
            // Find end of chunk size line.
            let Some(line_end) = data[pos..].windows(2).position(|w| w == b"\r\n") else {
                break;
            };
            let line_end = pos + line_end;
            let size_line = &data[pos..line_end];
            let size = usize::from_str_radix(std::str::from_utf8(size_line).unwrap_or("0"), 16).unwrap_or(0);
            if size == 0 {
                break;
            }
            let chunk_start = line_end + 2; // skip \r\n
            if chunk_start + size > data.len() {
                break;
            }
            result.extend_from_slice(&data[chunk_start..chunk_start + size]);
            pos = chunk_start + size + 2; // skip chunk data and \r\n
        }
        result
    }

    #[test]
    fn download_serves_file_content() {
        let dir = test_dest_dir();
        let file_path = create_test_file(&dir, "test.txt", b"Hello, lanbeam download!");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "dwntest";
        let state = Arc::new(Mutex::new(SessionState::default()));
        let share_paths = vec![file_path];

        let client = std::thread::spawn(move || download_request(addr, token, 0, None));

        let mut stream = accept_download(&listener);
        serve_download(
            &mut stream,
            &format!("/{}/download/0/test.txt", token),
            &state,
            &share_paths,
            false,
            None,
            None,
        )
        .expect("serve download");
        drop(stream);

        let (status, _headers, body) = client.join().expect("client joined");
        assert_eq!(status, 200);
        assert_eq!(body, b"Hello, lanbeam download!");
        assert_eq!(state.lock().unwrap().downloads_completed, 1);
    }

    #[test]
    fn download_supports_range_header() {
        let dir = test_dest_dir();
        let content = b"0123456789ABCDEF";
        let file_path = create_test_file(&dir, "test.txt", content);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "dwntest2";
        let state = Arc::new(Mutex::new(SessionState::default()));
        let share_paths = vec![file_path];

        // Request bytes 5-10 (inclusive).
        let client = std::thread::spawn(move || download_request(addr, token, 0, Some("bytes=5-10")));

        let mut stream = accept_download(&listener);
        serve_download(
            &mut stream,
            &format!("/{}/download/0/test.txt", token),
            &state,
            &share_paths,
            false,
            None,
            Some("bytes=5-10".to_string()),
        )
        .expect("serve download");
        drop(stream);

        let (status, headers, body) = client.join().expect("client joined");
        assert_eq!(status, 206);
        assert_eq!(body, b"56789A");
        let headers_str = String::from_utf8_lossy(&headers);
        assert!(headers_str.contains("Content-Range: bytes 5-10/16"));
        assert!(headers_str.contains("Accept-Ranges: bytes"));
    }

    #[test]
    fn download_supports_suffix_range() {
        let dir = test_dest_dir();
        let content = b"0123456789ABCDEF";
        let file_path = create_test_file(&dir, "test.txt", content);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "dwntest3";
        let state = Arc::new(Mutex::new(SessionState::default()));
        let share_paths = vec![file_path];

        // Request last 5 bytes.
        let client = std::thread::spawn(move || download_request(addr, token, 0, Some("bytes=-5")));

        let mut stream = accept_download(&listener);
        serve_download(
            &mut stream,
            &format!("/{}/download/0/test.txt", token),
            &state,
            &share_paths,
            false,
            None,
            Some("bytes=-5".to_string()),
        )
        .expect("serve download");
        drop(stream);

        let (status, _headers, body) = client.join().expect("client joined");
        assert_eq!(status, 206);
        assert_eq!(body, b"BCDEF");
    }

    #[test]
    fn download_supports_open_ended_range() {
        let dir = test_dest_dir();
        let content = b"0123456789ABCDEF";
        let file_path = create_test_file(&dir, "test.txt", content);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "dwntest4";
        let state = Arc::new(Mutex::new(SessionState::default()));
        let share_paths = vec![file_path];

        // Request from byte 10 to end.
        let client = std::thread::spawn(move || download_request(addr, token, 0, Some("bytes=10-")));

        let mut stream = accept_download(&listener);
        serve_download(
            &mut stream,
            &format!("/{}/download/0/test.txt", token),
            &state,
            &share_paths,
            false,
            None,
            Some("bytes=10-".to_string()),
        )
        .expect("serve download");
        drop(stream);

        let (status, _headers, body) = client.join().expect("client joined");
        assert_eq!(status, 206);
        assert_eq!(body, b"ABCDEF");
    }

    #[test]
    fn download_returns_416_for_unsatisfiable_range() {
        let dir = test_dest_dir();
        let content = b"0123456789ABCDEF";
        let file_path = create_test_file(&dir, "test.txt", content);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "dwntest5";
        let state = Arc::new(Mutex::new(SessionState::default()));
        let share_paths = vec![file_path];

        // Request bytes beyond file size.
        let client = std::thread::spawn(move || download_request(addr, token, 0, Some("bytes=100-200")));

        let mut stream = accept_download(&listener);
        serve_download(
            &mut stream,
            &format!("/{}/download/0/test.txt", token),
            &state,
            &share_paths,
            false,
            None,
            Some("bytes=100-200".to_string()),
        )
        .expect("serve download");
        drop(stream);

        let (status, headers, body) = client.join().expect("client joined");
        assert_eq!(status, 416);
        assert_eq!(body, b"");
        let headers_str = String::from_utf8_lossy(&headers);
        assert!(headers_str.contains("Content-Range: bytes */16"));
    }

    #[test]
    fn download_once_stops_after_first() {
        let dir = test_dest_dir();
        let file_path = create_test_file(&dir, "test.txt", b"content");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "dwntest6";
        let state = Arc::new(Mutex::new(SessionState::default()));
        let share_paths = vec![file_path];

        // First download.
        let client1 = std::thread::spawn(move || download_request(addr, token, 0, None));
        let mut stream = accept_download(&listener);
        serve_download(
            &mut stream,
            &format!("/{}/download/0/test.txt", token),
            &state,
            &share_paths,
            true, // once = true
            None,
            None,
        )
        .expect("serve download");
        drop(stream);
        client1.join().expect("client joined");
        
        // Session should be deactivated.
        assert!(!state.lock().unwrap().active);
        assert_eq!(state.lock().unwrap().downloads_completed, 1);
    }

    #[test]
    fn download_max_downloads_stops_after_n() {
        let dir = test_dest_dir();
        let file_path = create_test_file(&dir, "test.txt", b"content");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "dwntest7";
        let state = Arc::new(Mutex::new(SessionState::default()));
        let share_paths = vec![file_path];

        // First download - should not stop (max=2).
        let client1 = std::thread::spawn(move || download_request(addr, token, 0, None));
        let mut stream = accept_download(&listener);
        serve_download(
            &mut stream,
            &format!("/{}/download/0/test.txt", token),
            &state,
            &share_paths,
            false,
            Some(2), // max_downloads = 2
            None,
        )
        .expect("serve download");
        drop(stream);
        client1.join().expect("client joined");
        assert!(state.lock().unwrap().active);
        assert_eq!(state.lock().unwrap().downloads_completed, 1);

        // Second download - should stop (reached max).
        let client2 = std::thread::spawn(move || download_request(addr, token, 0, None));
        let mut stream = accept_download(&listener);
        serve_download(
            &mut stream,
            &format!("/{}/download/0/test.txt", token),
            &state,
            &share_paths,
            false,
            Some(2),
            None,
        )
        .expect("serve download");
        drop(stream);
        client2.join().expect("client joined");
        assert!(!state.lock().unwrap().active);
        assert_eq!(state.lock().unwrap().downloads_completed, 2);
    }

    #[test]
    fn download_returns_404_for_invalid_index() {
        let dir = test_dest_dir();
        let file_path = create_test_file(&dir, "test.txt", b"content");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "dwntest8";
        let state = Arc::new(Mutex::new(SessionState::default()));
        let share_paths = vec![file_path];

        let client = std::thread::spawn(move || download_request(addr, token, 5, None));
        let mut stream = accept_download(&listener);
        serve_download(
            &mut stream,
            &format!("/{}/download/5/test.txt", token),
            &state,
            &share_paths,
            false,
            None,
            None,
        )
        .expect("serve download");
        drop(stream);

        let (status, _headers, _body) = client.join().expect("client joined");
        assert_eq!(status, 404);
    }

    // --- download-all.zip ---

    #[test]
    fn download_all_creates_zip_for_multiple_files() {
        let dir = test_dest_dir();
        let file1 = create_test_file(&dir, "file1.txt", b"content1");
        let file2 = create_test_file(&dir, "file2.txt", b"content2");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "dwntest9";
        let share_paths = vec![file1, file2];

        let mut stream = TcpStream::connect(addr).expect("connect");
        let mut request = String::new();
        request.push_str(&format!("GET /{}/download-all.zip HTTP/1.1\r\n", token));
        request.push_str("host: localhost\r\n");
        request.push_str("connection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).expect("write request");

        let mut server_stream = accept_download(&listener);
        serve_download_all(&mut server_stream, &share_paths).expect("serve download all");
        drop(server_stream);

        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read response");
        let body_start = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| i + 4)
            .expect("headers");
        let zip_bytes = decode_chunked(&response[body_start..]);
        assert!(zip_bytes.starts_with(&crate::zip::ZIP_LOCAL_HEADER_SIG.to_le_bytes()));
        let zip_str = String::from_utf8_lossy(&zip_bytes);
        assert!(zip_str.contains("file1.txt"));
        assert!(zip_str.contains("file2.txt"));
        assert!(zip_str.contains("content1"));
        assert!(zip_str.contains("content2"));
    }

    #[test]
    fn download_all_creates_zip_for_directory() {
        let dir = test_dest_dir();
        let subdir = dir.join("subdir");
        fs::create_dir(&subdir).expect("create subdir");
        let _file1 = create_test_file(&dir, "root.txt", b"root");
        let _file2 = create_test_file(&subdir, "nested.txt", b"nested");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "dwntest10";
        let share_paths = vec![dir.clone()];

        let mut stream = TcpStream::connect(addr).expect("connect");
        let mut request = String::new();
        request.push_str(&format!("GET /{}/download-all.zip HTTP/1.1\r\n", token));
        request.push_str("host: localhost\r\n");
        request.push_str("connection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).expect("write request");

        let mut server_stream = accept_download(&listener);
        serve_download_all(&mut server_stream, &share_paths).expect("serve download all");
        drop(server_stream);

        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read response");
        let body_start = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| i + 4)
            .expect("headers");
        let zip_bytes = decode_chunked(&response[body_start..]);
        assert!(zip_bytes.starts_with(&crate::zip::ZIP_LOCAL_HEADER_SIG.to_le_bytes()));
        let zip_str = String::from_utf8_lossy(&zip_bytes);
        assert!(zip_str.contains("root.txt"));
        assert!(zip_str.contains("subdir/nested.txt"));
        assert!(zip_str.contains("root"));
        assert!(zip_str.contains("nested"));
    }

    #[test]
    fn download_all_returns_404_for_empty() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "dwntest11";
        let share_paths = vec![];

        let mut stream = TcpStream::connect(addr).expect("connect");
        let mut request = String::new();
        request.push_str(&format!("GET /{}/download-all.zip HTTP/1.1\r\n", token));
        request.push_str("host: localhost\r\n");
        request.push_str("connection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).expect("write request");

        let mut server_stream = accept_download(&listener);
        let result = serve_download_all(&mut server_stream, &share_paths);
        assert!(result.is_ok());
        drop(server_stream);

        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read response");
        let status_line_end = response.windows(2).position(|w| w == b"\r\n").unwrap_or(0);
        let status_line = String::from_utf8_lossy(&response[..status_line_end]);
        let status_code = status_line.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0);
        assert_eq!(status_code, 404);
    }

    // --- parse_range_header unit tests ---

    #[test]
    fn parse_range_header_basic() {
        assert_eq!(parse_range_header(Some("bytes=0-9".to_string()), 100), Some((0, 9)));
        assert_eq!(parse_range_header(Some("bytes=10-19".to_string()), 100), Some((10, 19)));
        assert_eq!(parse_range_header(Some("bytes=50-".to_string()), 100), Some((50, 99)));
        assert_eq!(parse_range_header(Some("bytes=-10".to_string()), 100), Some((90, 99)));
    }

    #[test]
    fn parse_range_header_edge_cases() {
        // Suffix larger than file.
        assert_eq!(parse_range_header(Some("bytes=-200".to_string()), 100), Some((0, 99)));
        // Exact file size.
        assert_eq!(parse_range_header(Some("bytes=0-99".to_string()), 100), Some((0, 99)));
        // Single byte.
        assert_eq!(parse_range_header(Some("bytes=50-50".to_string()), 100), Some((50, 50)));
    }

    #[test]
    fn parse_range_header_invalid() {
        assert_eq!(parse_range_header(None, 100), None);
        assert_eq!(parse_range_header(Some("invalid".to_string()), 100), None);
        assert_eq!(parse_range_header(Some("bytes=".to_string()), 100), None);
        assert_eq!(parse_range_header(Some("bytes=abc-def".to_string()), 100), None);
        assert_eq!(parse_range_header(Some("bytes=-0".to_string()), 100), None);
    }

    // --- end-to-end routing & security tests ---

    fn send_raw_http(addr: SocketAddr, request: &[u8]) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream.write_all(request).expect("write request");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read response");
        let text = String::from_utf8_lossy(&response).into_owned();
        let status_code = text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|c| c.parse::<u16>().ok())
            .unwrap_or(0);
        (status_code, text)
    }

    #[test]
    fn route_upload_with_query_param_and_max_uploads_end_to_end() {
        let dir = test_dest_dir();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let port = addr.port();
        let token = "secTok123";
        let session = ServerSession::new(listener, port, token)
            .expect("session")
            .with_dest(dir.clone())
            .with_max_uploads(Some(2));
        let state = session.state.clone();
        let server_thread = thread::spawn(move || session.run());

        // First upload with percent-encoded space and query string.
        let req1 = format!(
            "POST /{token}/upload?name=photo%201.jpg HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello"
        );
        let (s1, body1) = send_raw_http(addr, req1.as_bytes());
        assert_eq!(s1, 200, "response: {body1}");
        assert!(dir.join("photo 1.jpg").is_file());
        assert!(lock_state(&state).active);

        // Second upload hits max_uploads=2 and cleanly shuts down the session.
        let req2 = format!(
            "POST /{token}/upload?name=photo%201.jpg HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nworld"
        );
        let (s2, body2) = send_raw_http(addr, req2.as_bytes());
        assert_eq!(s2, 200, "response: {body2}");
        assert!(dir.join("photo 1_1.jpg").is_file());
        assert_eq!(fs::read(dir.join("photo 1.jpg")).unwrap(), b"hello");
        assert_eq!(fs::read(dir.join("photo 1_1.jpg")).unwrap(), b"world");
        assert_eq!(lock_state(&state).uploads_received, 2);

        server_thread.join().expect("server thread join").expect("server ok");
        assert!(!lock_state(&state).active);
    }

    #[test]
    fn route_rejects_root_and_token_prefix_collision_with_403() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let port = addr.port();
        let token = "tokABC";
        let session = ServerSession::new(listener, port, token).expect("session");
        let state = session.state.clone();
        let server_thread = thread::spawn(move || session.run());

        let (s_root, _) = send_raw_http(addr, b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(s_root, 403);

        let (s_prefix, _) = send_raw_http(
            addr,
            format!("GET /{token}extra/status HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
        );
        assert_eq!(s_prefix, 403);

        let (s_wrong, _) = send_raw_http(addr, b"GET /wrong/status HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(s_wrong, 403);

        let (s_stop, _) = send_raw_http(
            addr,
            format!("POST /{token}/stop?confirm=1 HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
        );
        assert_eq!(s_stop, 200);

        server_thread.join().expect("server thread join").expect("server ok");
        assert!(!lock_state(&state).active);
    }

    #[test]
    fn upload_incomplete_body_removes_part_and_missing_length_returns_411() {
        let dir = test_dest_dir();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let port = addr.port();
        let token = "upguard";
        let session = ServerSession::new(listener, port, token)
            .expect("session")
            .with_dest(dir.clone());
        let state = session.state.clone();
        let server_thread = thread::spawn(move || session.run());

        // Missing Content-Length -> 411 Length Required.
        let (s_411, _) = send_raw_http(
            addr,
            format!("POST /{token}/upload?name=no_len.txt HTTP/1.1\r\nHost: localhost\r\n\r\nabc")
                .as_bytes(),
        );
        assert_eq!(s_411, 411);
        assert!(!dir.join("no_len.txt").exists());

        // Short body (Content-Length: 100, only 5 bytes sent) -> 400 and .part deleted.
        let mut stream = TcpStream::connect(addr).expect("connect");
        let partial_req = format!(
            "POST /{token}/upload?name=trunc.bin HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\n\r\n12345"
        );
        stream.write_all(partial_req.as_bytes()).expect("write partial");
        stream.shutdown(std::net::Shutdown::Write).expect("shutdown write");
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).expect("read response");
        let resp_str = String::from_utf8_lossy(&resp);
        assert!(resp_str.starts_with("HTTP/1.1 400"), "resp: {resp_str}");
        assert!(!dir.join("trunc.bin").exists());
        assert!(!dir.join(".trunc.bin.lanbeam.part").exists());
        assert_eq!(lock_state(&state).uploads_received, 0);

        let _ = send_raw_http(
            addr,
            format!("POST /{token}/stop HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
        );
        server_thread.join().expect("server thread join").expect("server ok");
    }

    #[cfg(unix)]
    #[test]
    fn download_all_skips_symlinks_and_prevents_loop() {
        use std::os::unix::fs::symlink;

        let outer = test_dest_dir();
        let secret = create_test_file(&outer, "secret.txt", b"TOP_SECRET_OUTSIDE");
        let shared = outer.join("shared");
        let sub = shared.join("sub");
        fs::create_dir_all(&sub).expect("create shared/sub");
        let _real = create_test_file(&shared, "public.txt", b"PUBLIC_OK");

        // Symlink pointing outside shared dir, plus a directory loop symlink.
        symlink(&secret, shared.join("leak.txt")).expect("symlink leak");
        symlink(&shared, sub.join("loop")).expect("symlink loop");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let port = addr.port();
        let token = "symlk";
        let session = ServerSession::new(listener, port, token)
            .expect("session")
            .with_share_paths(vec![shared]);
        let server_thread = thread::spawn(move || session.run());

        let req = format!("GET /{token}/download-all.zip HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream.write_all(req.as_bytes()).expect("write");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read");
        let body_start = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| i + 4)
            .expect("headers");
        let zip_bytes = decode_chunked(&response[body_start..]);
        let zip_str = String::from_utf8_lossy(&zip_bytes);
        assert!(zip_str.contains("public.txt"));
        assert!(zip_str.contains("PUBLIC_OK"));
        assert!(!zip_str.contains("leak.txt"));
        assert!(!zip_str.contains("TOP_SECRET_OUTSIDE"));

        let _ = send_raw_http(
            addr,
            format!("POST /{token}/stop HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
        );
        server_thread.join().expect("server thread join").expect("server ok");
    }

    #[test]
    fn partial_range_probe_does_not_trip_once_before_end() {
        let dir = test_dest_dir();
        let file_path = create_test_file(&dir, "video.mp4", b"0123456789ABCDEF");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let token = "rangeonce";
        let state = Arc::new(Mutex::new(SessionState::default()));
        let share_paths = vec![file_path];

        // Partial range 0-3 does not reach end of file (byte 15).
        let client1 = std::thread::spawn(move || download_request(addr, token, 0, Some("bytes=0-3")));
        let mut stream = accept_download(&listener);
        serve_download(
            &mut stream,
            &format!("/{}/download/0/video.mp4", token),
            &state,
            &share_paths,
            true,
            None,
            Some("bytes=0-3".to_string()),
        )
        .expect("serve partial");
        drop(stream);
        let (s1, _, b1) = client1.join().expect("join1");
        assert_eq!(s1, 206);
        assert_eq!(b1, b"0123");
        assert!(lock_state(&state).active);
        assert_eq!(lock_state(&state).downloads_completed, 0);

        // Final range 4-15 reaches byte 15 (file_len - 1), completing the download.
        let client2 = std::thread::spawn(move || download_request(addr, token, 0, Some("bytes=4-15")));
        let mut stream = accept_download(&listener);
        serve_download(
            &mut stream,
            &format!("/{}/download/0/video.mp4", token),
            &state,
            &share_paths,
            true,
            None,
            Some("bytes=4-15".to_string()),
        )
        .expect("serve final");
        drop(stream);
        let (s2, _, b2) = client2.join().expect("join2");
        assert_eq!(s2, 206);
        assert_eq!(b2, b"456789ABCDEF");
        assert!(!lock_state(&state).active);
        assert_eq!(lock_state(&state).downloads_completed, 1);
    }

    #[test]
    fn escape_json_escapes_control_characters() {
        assert_eq!(escape_json("a\nb\rc\t\"d\\"), "a\\nb\\rc\\t\\\"d\\\\");
        assert_eq!(escape_json("\u{0007}"), "\\u0007");
    }
}