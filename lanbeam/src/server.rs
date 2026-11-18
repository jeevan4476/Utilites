//! Multi-threaded streaming HTTP server (Step 6).
//!
//! Implements `ServerSession` over `std::net::TcpListener` serving the lanbeam
//! transfer app on `GET /<token>` with file download, upload, text, and QR
//! endpoints.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::qr::QrCode;
use crate::web_assets::WEB_ASSETS;

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
    // Additional fields for Half 1 & Half 2
    pub shared_paths: Vec<PathBuf>,
    pub zip_roots: Vec<PathBuf>,
    pub once: bool,
    pub max_downloads: Option<u32>,
    pub dest_dir: PathBuf,
    pub max_uploads: Option<u32>,
    pub timeout_secs: Option<u64>,
    pub started_at: Instant,
    pub last_activity: Instant,
}

impl Default for SessionState {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            mode: "send".to_string(),
            url: String::new(),
            files: Vec::new(),
            downloads_completed: 0,
            uploads_received: 0,
            shared_text: None,
            remaining_seconds: 0,
            active: true,
            shared_paths: Vec::new(),
            zip_roots: Vec::new(),
            once: false,
            max_downloads: None,
            dest_dir: PathBuf::new(),
            max_uploads: None,
            timeout_secs: None,
            started_at: now,
            last_activity: now,
        }
    }
}

/// An HTTP server session bound to a single token.
pub struct ServerSession {
    listener: TcpListener,
    token: String,
    state: Arc<Mutex<SessionState>>,
    port: u16,
}

impl ServerSession {
    /// Create a new session bound to `listener` on `port` serving `token`.
    pub fn new(listener: TcpListener, port: u16, token: &str) -> io::Result<Self> {
        Ok(Self {
            listener,
            token: token.to_string(),
            state: Arc::new(Mutex::new(SessionState::default())),
            port,
        })
    }

    /// The TCP port the listener is bound to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The session token.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Configure for send mode (file sharing).
    pub fn configure_send(
        &self,
        paths: Vec<PathBuf>,
        once: bool,
        max_downloads: Option<u32>,
        timeout_secs: Option<u64>,
        url: String,
    ) {
        let mut expanded_paths = Vec::new();
        let mut file_names = Vec::new();
        for p in &paths {
            if p.is_dir() {
                let mut collected = Vec::new();
                if collect_files(p, p, &mut collected).is_ok() {
                    collected.sort_by(|a, b| a.0.cmp(&b.0));
                    for (rel, file_path) in collected {
                        // Use relative path with forward slashes for file_names
                        let rel_forward = rel.replace('\\', "/");
                        file_names.push(rel_forward);
                        expanded_paths.push(file_path);
                    }
                }
            } else {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    file_names.push(name.to_string());
                }
                expanded_paths.push(p.clone());
            }
        }

        let now = Instant::now();
        let mut st = lock_state(&self.state);
        st.mode = "send".to_string();
        st.zip_roots = paths;
        st.files = file_names;
        st.shared_paths = expanded_paths;
        st.once = once;
        st.max_downloads = max_downloads;
        st.timeout_secs = timeout_secs;
        st.remaining_seconds = timeout_secs.unwrap_or(0);
        st.started_at = now;
        st.last_activity = now;
        st.url = url;
    }

    /// Configure for receive mode (file upload).
    pub fn configure_receive(
        &self,
        dest_dir: PathBuf,
        max_uploads: Option<u32>,
        timeout_secs: Option<u64>,
        url: String,
    ) {
        let now = Instant::now();
        let mut st = lock_state(&self.state);
        st.mode = "receive".to_string();
        st.dest_dir = dest_dir;
        st.max_uploads = max_uploads;
        st.timeout_secs = timeout_secs;
        st.remaining_seconds = timeout_secs.unwrap_or(0);
        st.started_at = now;
        st.last_activity = now;
        st.url = url;
    }

    /// Configure for text mode.
    pub fn configure_text(
        &self,
        shared_text: Option<String>,
        once: bool,
        timeout_secs: Option<u64>,
        url: String,
    ) {
        let now = Instant::now();
        let mut st = lock_state(&self.state);
        st.mode = "text".to_string();
        st.shared_text = shared_text;
        st.once = once;
        st.timeout_secs = timeout_secs;
        st.remaining_seconds = timeout_secs.unwrap_or(0);
        st.started_at = now;
        st.last_activity = now;
        st.url = url;
    }

    /// Run the listener loop, handling each connection on its own thread until
    /// `stop()` is called or the session is deactivated.
    pub fn run(self) -> io::Result<()> {
        const MAX_ACTIVE_WORKERS: usize = 64;
        self.listener.set_nonblocking(true)?;
        let mut workers: Vec<thread::JoinHandle<()>> = Vec::new();

        loop {
            // Reap finished worker threads periodically
            let mut i = 0;
            while i < workers.len() {
                if workers[i].is_finished() {
                    let handle = workers.swap_remove(i);
                    let _ = handle.join();
                } else {
                    i += 1;
                }
            }

            if workers.len() >= MAX_ACTIVE_WORKERS {
                if !lock_state(&self.state).active {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
                continue;
            }

            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    let state = self.state.clone();
                    let token = self.token.clone();
                    let handle = thread::spawn(move || {
                        if let Err(e) = handle_connection(stream, &token, &state) {
                            eprintln!("lanbeam: connection error: {e}");
                        }
                    });
                    workers.push(handle);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    let (active, timeout_expired) = {
                        let mut st = lock_state(&self.state);
                        if let Some(timeout) = st.timeout_secs {
                            let elapsed = st.last_activity.elapsed().as_secs();
                            if elapsed >= timeout {
                                st.active = false;
                                st.remaining_seconds = 0;
                                (false, true)
                            } else {
                                st.remaining_seconds = timeout.saturating_sub(elapsed);
                                (st.active, false)
                            }
                        } else {
                            (st.active, false)
                        }
                    };
                    if !active || timeout_expired {
                        break;
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Err(e) => return Err(e),
            }
        }

        // Join all remaining worker threads
        for w in workers {
            let _ = w.join();
        }
        Ok(())
    }

    /// Cleanly terminate the listener loop.
    pub fn stop(&self) {
        lock_state(&self.state).active = false;
    }
}

/// Poison-safe lock helper.
fn lock_state(state: &Mutex<SessionState>) -> std::sync::MutexGuard<'_, SessionState> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Update last_activity timestamp on any transfer activity.
fn touch_activity(state: &Arc<Mutex<SessionState>>) {
    let mut st = lock_state(state);
    st.last_activity = Instant::now();
}

/// Handle a single HTTP connection with raw-byte header reading and strict token routing.
fn handle_connection(
    mut stream: TcpStream,
    token: &str,
    state: &Arc<Mutex<SessionState>>,
) -> io::Result<()> {
    // Set timeouts
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;

    // Read headers into a buffer until \r\n\r\n is found
    let mut buf = Vec::with_capacity(4096);
    let mut temp = [0u8; 4096];
    let mut header_end = None;

    loop {
        match stream.read(&mut temp) {
            Ok(0) => {
                // EOF before headers complete
                if header_end.is_none() {
                    return Ok(());
                }
                break;
            }
            Ok(n) => {
                let prev_len = buf.len();
                buf.extend_from_slice(&temp[..n]);
                // Check only newly arrived boundary for header terminator
                if header_end.is_none() {
                    let search_start = prev_len.saturating_sub(3);
                    if let Some(pos) = buf[search_start..]
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                    {
                        let abs_pos = search_start + pos;
                        if abs_pos > 16 * 1024 {
                            return respond(
                                &mut stream,
                                400,
                                "text/plain",
                                b"Bad Request: Headers too large",
                            );
                        }
                        header_end = Some(abs_pos);
                        break;
                    }
                }
                if buf.len() > 16 * 1024 && header_end.is_none() {
                    return respond(
                        &mut stream,
                        400,
                        "text/plain",
                        b"Bad Request: Headers too large",
                    );
                }
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                return respond(&mut stream, 408, "text/plain", b"Request Timeout");
            }
            Err(e) => return Err(e),
        }
    }

    let header_end = header_end
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "Incomplete headers"))?;

    // Split headers and initial body
    let header_bytes = &buf[..header_end];
    let initial_body = &buf[header_end + 4..];

    // Parse headers as UTF-8
    let header_str = match std::str::from_utf8(header_bytes) {
        Ok(s) => s,
        Err(_) => {
            return respond(
                &mut stream,
                400,
                "text/plain",
                b"Bad Request: Invalid UTF-8 in headers",
            )
        }
    };

    // Parse request line
    let mut lines = header_str.lines();
    let request_line = lines.next().unwrap_or("");
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return respond(&mut stream, 400, "text/plain", b"Bad Request");
    }
    let method = parts[0];
    let full_path = parts[1];

    // Split path on first '?' to get raw_path and query
    let (raw_path, query) = match full_path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (full_path, None),
    };

    // Strict token boundary check (NO exemption for "/")
    let expected_prefix = format!("/{}", token);
    let is_valid_token_path = match raw_path.strip_prefix(&expected_prefix) {
        Some("") => true,
        Some(rest) => rest.starts_with('/'),
        None => false,
    };
    if !is_valid_token_path {
        return respond(&mut stream, 403, "text/plain", b"Forbidden");
    }

    // Route based on raw_path and method
    match (method, raw_path) {
        ("GET", p) if p == &expected_prefix || p == format!("{expected_prefix}/") => {
            serve_app(&mut stream, state, token)
        }
        ("GET", p) if p == format!("{expected_prefix}/qr.svg") => {
            serve_qr(&mut stream, state, token)
        }
        ("GET", p) if p == format!("{expected_prefix}/status") => {
            serve_status(&mut stream, state, token)
        }
        ("GET", p) if p.starts_with(&format!("{expected_prefix}/download/")) => {
            serve_download(&mut stream, p, state, token, header_str)
        }
        ("GET", p) if p == format!("{expected_prefix}/download-all.zip") => {
            serve_download_all(&mut stream, state)
        }
        ("POST", p) if p == format!("{expected_prefix}/upload") => {
            serve_upload(&mut stream, state, query, initial_body, header_str)
        }
        ("POST", p) if p == format!("{expected_prefix}/text") => {
            // Read body for text endpoint - support both Content-Length and chunked encoding
            if has_transfer_encoding(header_str)
                && (!is_chunked_transfer(header_str) || get_content_length(header_str).is_some())
            {
                return respond(
                    &mut stream,
                    400,
                    "text/plain",
                    b"Bad Request: Unsupported or ambiguous Transfer-Encoding",
                );
            }
            if is_chunked_transfer(header_str) {
                // Chunked encoding path - max 64 KiB
                let mut body = Vec::new();
                let result =
                    read_chunked_body(&mut stream, initial_body, state, 64 * 1024, |chunk| {
                        touch_activity(state);
                        body.extend_from_slice(chunk);
                        Ok(())
                    });
                match result {
                    Ok(_) => {
                        // Validate strict UTF-8
                        let body_str = match std::str::from_utf8(&body) {
                            Ok(s) => s,
                            Err(_) => {
                                return respond(
                                    &mut stream,
                                    400,
                                    "text/plain",
                                    b"Bad Request: Invalid UTF-8 in body",
                                )
                            }
                        };
                        serve_text(&mut stream, state, body_str)
                    }
                    Err((status, message)) => {
                        respond(&mut stream, status, "text/plain", message.as_bytes())
                    }
                }
            } else {
                // Content-Length path
                let content_length = match header_str
                    .lines()
                    .find(|l| l.to_lowercase().starts_with("content-length:"))
                    .and_then(|l| l.split(':').nth(1))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                {
                    Some(cl) => cl,
                    None => {
                        return respond(
                            &mut stream,
                            400,
                            "text/plain",
                            b"Bad Request: Missing Content-Length",
                        )
                    }
                };

                if content_length > 64 * 1024 {
                    return respond(
                        &mut stream,
                        400,
                        "text/plain",
                        b"Bad Request: Content-Length exceeds 64 KiB",
                    );
                }

                let mut body = initial_body[..initial_body.len().min(content_length)].to_vec();
                while body.len() < content_length {
                    let remaining = content_length - body.len();
                    let chunk_cap = temp.len().min(remaining);
                    let n = stream.read(&mut temp[..chunk_cap])?;
                    if n == 0 {
                        return respond(
                            &mut stream,
                            400,
                            "text/plain",
                            b"Bad Request: Premature EOF",
                        );
                    }
                    touch_activity(state);
                    body.extend_from_slice(&temp[..n]);
                }

                // Validate strict UTF-8
                let body_str = match std::str::from_utf8(&body) {
                    Ok(s) => s,
                    Err(_) => {
                        return respond(
                            &mut stream,
                            400,
                            "text/plain",
                            b"Bad Request: Invalid UTF-8 in body",
                        )
                    }
                };

                serve_text(&mut stream, state, body_str)
            }
        }
        ("POST", p) if p == format!("{expected_prefix}/stop") => {
            lock_state(state).active = false;
            respond(&mut stream, 200, "text/plain", b"Stopping")
        }
        _ => respond(&mut stream, 404, "text/plain", b"Not Found"),
    }
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        status_reason(status),
        content_type,
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    if !body.is_empty() {
        stream.write_all(body)?;
    }
    Ok(())
}

fn get_content_length(headers: &str) -> Option<usize> {
    headers.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}

/// Check if Transfer-Encoding header indicates chunked encoding.
fn is_chunked_transfer(headers: &str) -> bool {
    let encodings: Vec<_> = headers
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("transfer-encoding"))
        .map(|(_, value)| value.trim())
        .collect();
    encodings.len() == 1 && encodings[0].eq_ignore_ascii_case("chunked")
}

fn has_transfer_encoding(headers: &str) -> bool {
    headers
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .any(|(name, _)| name.trim().eq_ignore_ascii_case("transfer-encoding"))
}

/// Read a chunked request body with streaming processing.
/// Returns total bytes processed or an error with status code and message.
fn read_chunked_body<F>(
    stream: &mut TcpStream,
    initial_body: &[u8],
    state: &Arc<Mutex<SessionState>>,
    max_bytes: u64,
    mut on_chunk: F,
) -> Result<u64, (u16, &'static str)>
where
    F: FnMut(&[u8]) -> Result<(), (u16, &'static str)>,
{
    const IO_CHUNK_SIZE: usize = 64 * 1024;
    let mut buffer = Vec::with_capacity(IO_CHUNK_SIZE);
    buffer.extend_from_slice(initial_body);
    let mut total_bytes = 0u64;
    let mut temp = [0u8; IO_CHUNK_SIZE];

    loop {
        let line_end = buffer.windows(2).position(|w| w == b"\r\n");
        if line_end.is_none() {
            if buffer.len() > 8192 {
                return Err((400, "Bad Request: Chunk size line too long"));
            }
            match stream.read(&mut temp) {
                Ok(0) => return Err((400, "Bad Request: Incomplete chunked body")),
                Ok(n) => {
                    touch_activity(state);
                    buffer.extend_from_slice(&temp[..n]);
                    continue;
                }
                Err(e)
                    if e.kind() == io::ErrorKind::TimedOut
                        || e.kind() == io::ErrorKind::WouldBlock =>
                {
                    return Err((400, "Bad Request: Premature EOF"));
                }
                Err(_) => return Err((500, "Internal Server Error: Read failed")),
            }
        }
        let line_end = line_end.unwrap();
        if line_end > 8192 {
            return Err((400, "Bad Request: Chunk size line too long"));
        }
        let size_line = std::str::from_utf8(&buffer[..line_end])
            .map_err(|_| (400, "Bad Request: Invalid chunk size encoding"))?;
        let size_str = size_line.split(';').next().unwrap_or("").trim();
        let chunk_size = u64::from_str_radix(size_str, 16)
            .map_err(|_| (400, "Bad Request: Invalid chunk size"))?;
        buffer.drain(..line_end + 2);

        if chunk_size == 0 {
            let mut trailer_bytes = 0usize;
            loop {
                let trailer_end = buffer.windows(2).position(|w| w == b"\r\n");
                if trailer_end.is_none() {
                    if buffer.len() > 8192 || trailer_bytes.saturating_add(buffer.len()) > 64 * 1024
                    {
                        return Err((400, "Bad Request: Chunk trailers too large"));
                    }
                    match stream.read(&mut temp) {
                        Ok(0) => return Err((400, "Bad Request: Incomplete chunk trailers")),
                        Ok(n) => {
                            touch_activity(state);
                            buffer.extend_from_slice(&temp[..n]);
                            continue;
                        }
                        Err(e)
                            if e.kind() == io::ErrorKind::TimedOut
                                || e.kind() == io::ErrorKind::WouldBlock =>
                        {
                            return Err((400, "Bad Request: Premature EOF"));
                        }
                        Err(_) => return Err((500, "Internal Server Error: Read failed")),
                    }
                }
                let trailer_end = trailer_end.unwrap();
                if trailer_end == 0 {
                    buffer.drain(..2);
                    if !buffer.is_empty() {
                        return Err((400, "Bad Request: Unexpected bytes after chunked body"));
                    }
                    return Ok(total_bytes);
                }
                if trailer_end > 8192 || trailer_bytes.saturating_add(trailer_end + 2) > 64 * 1024 {
                    return Err((400, "Bad Request: Chunk trailers too large"));
                }
                trailer_bytes += trailer_end + 2;
                buffer.drain(..trailer_end + 2);
            }
        }

        let new_total = total_bytes
            .checked_add(chunk_size)
            .ok_or((413, "Payload Too Large"))?;
        if new_total > max_bytes {
            return Err((413, "Payload Too Large"));
        }
        let mut remaining = usize::try_from(chunk_size).map_err(|_| (413, "Payload Too Large"))?;
        while remaining > 0 {
            if !buffer.is_empty() {
                let count = remaining.min(buffer.len());
                on_chunk(&buffer[..count]).map_err(|e| e)?;
                buffer.drain(..count);
                remaining -= count;
                continue;
            }
            let count = remaining.min(temp.len());
            match stream.read(&mut temp[..count]) {
                Ok(0) => return Err((400, "Bad Request: Premature EOF")),
                Ok(n) => {
                    touch_activity(state);
                    on_chunk(&temp[..n]).map_err(|e| e)?;
                    remaining -= n;
                }
                Err(e)
                    if e.kind() == io::ErrorKind::TimedOut
                        || e.kind() == io::ErrorKind::WouldBlock =>
                {
                    return Err((400, "Bad Request: Premature EOF"));
                }
                Err(_) => return Err((500, "Internal Server Error: Read failed")),
            }
        }
        total_bytes = new_total;

        while buffer.len() < 2 {
            let missing = 2 - buffer.len();
            match stream.read(&mut temp[..missing]) {
                Ok(0) => return Err((400, "Bad Request: Premature EOF")),
                Ok(n) => {
                    touch_activity(state);
                    buffer.extend_from_slice(&temp[..n]);
                }
                Err(e)
                    if e.kind() == io::ErrorKind::TimedOut
                        || e.kind() == io::ErrorKind::WouldBlock =>
                {
                    return Err((400, "Bad Request: Premature EOF"));
                }
                Err(_) => return Err((500, "Internal Server Error: Read failed")),
            }
        }
        if &buffer[..2] != b"\r\n" {
            return Err((400, "Bad Request: Missing CRLF after chunk data"));
        }
        buffer.drain(..2);
    }
}

fn respond_headers(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> io::Result<()> {
    let mut response = format!("HTTP/1.1 {} {}\r\n", status, status_reason(status));
    for (k, v) in headers {
        response.push_str(k);
        response.push_str(": ");
        response.push_str(v);
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    stream.write_all(response.as_bytes())?;
    if let Some(b) = body {
        stream.write_all(b)?;
    }
    Ok(())
}

fn serialize_session_json(st: &SessionState, token: Option<&str>) -> String {
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
    let token_prefix = match token {
        Some(t) => format!("\"token\":\"{}\",", escape_json(t)),
        None => String::new(),
    };
    format!(
        "{{{}\"mode\":\"{}\",\"url\":\"{}\",\"files\":[{}],\"downloads_completed\":{},\"uploads_received\":{},{},\"remaining_seconds\":{},\"active\":{}}}",
        token_prefix,
        escape_json(&st.mode),
        escape_json(&st.url),
        files_json,
        st.downloads_completed,
        st.uploads_received,
        shared_text_json,
        st.remaining_seconds,
        st.active
    )
}

fn serve_app(
    stream: &mut TcpStream,
    state: &Arc<Mutex<SessionState>>,
    token: &str,
) -> io::Result<()> {
    let status_json = {
        let st = lock_state(state);
        serialize_session_json(&st, Some(token))
    }
    .replace('<', "\\u003c");
    let injection = format!(
        "<script>window.__LANBEAM_INITIAL_STATE__ = {};</script>",
        status_json
    );
    let total_len = (WEB_ASSETS.len() + injection.len()).to_string();
    respond_headers(
        stream,
        200,
        &[
            ("Content-Type", "text/html; charset=utf-8"),
            ("Content-Length", &total_len),
            ("Connection", "close"),
        ],
        None,
    )?;

    let assets = WEB_ASSETS.as_bytes();
    if let Some(pos) = WEB_ASSETS
        .rfind("</head>")
        .or_else(|| WEB_ASSETS.find("</body>"))
    {
        stream.write_all(&assets[..pos])?;
        stream.write_all(injection.as_bytes())?;
        stream.write_all(&assets[pos..])?;
    } else {
        stream.write_all(assets)?;
        stream.write_all(injection.as_bytes())?;
    }
    Ok(())
}

fn serve_qr(
    stream: &mut TcpStream,
    state: &Arc<Mutex<SessionState>>,
    token: &str,
) -> io::Result<()> {
    let url = {
        let st = lock_state(state);
        if st.url.is_empty() {
            token.to_string()
        } else {
            st.url.clone()
        }
    };
    let qr = match QrCode::encode(&url) {
        Ok(q) => q,
        Err(_) => {
            return respond(
                stream,
                500,
                "text/plain",
                b"Internal Server Error: QR encode failed",
            )
        }
    };
    let svg = qr.to_svg(4);
    respond(stream, 200, "image/svg+xml", svg.as_bytes())
}

fn serve_status(
    stream: &mut TcpStream,
    state: &Arc<Mutex<SessionState>>,
    token: &str,
) -> io::Result<()> {
    let body = {
        let st = lock_state(state);
        serialize_session_json(&st, Some(token))
    };
    respond(stream, 200, "application/json", body.as_bytes())
}

fn escape_json(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if (c as u32) < 0x20 => escaped.push_str(&format!("\\u{:04x}", c as u32)),
            c => escaped.push(c),
        }
    }
    escaped
}

/// Percent-decode a path component from bytes, returning Result<&str, ()>.
/// Validates UTF-8 and rejects null bytes and control characters.
fn percent_decode(s: &str) -> Result<String, ()> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            if i + 2 >= bytes.len() {
                return Err(()); // incomplete
            }
            let hi = bytes[i + 1];
            let lo = bytes[i + 2];
            let hi_val = match hi {
                b'0'..=b'9' => hi - b'0',
                b'A'..=b'F' => hi - b'A' + 10,
                b'a'..=b'f' => hi - b'a' + 10,
                _ => return Err(()), // invalid hex
            };
            let lo_val = match lo {
                b'0'..=b'9' => lo - b'0',
                b'A'..=b'F' => lo - b'A' + 10,
                b'a'..=b'f' => lo - b'a' + 10,
                _ => return Err(()), // invalid hex
            };
            let byte = (hi_val << 4) | lo_val;
            if byte == 0 || byte.is_ascii_control() {
                return Err(()); // null or control
            }
            out.push(byte);
            i += 3;
        } else if b == b'\\' {
            return Err(()); // backslash
        } else {
            out.push(b);
            i += 1;
        }
    }
    // Validate UTF-8
    let s = std::str::from_utf8(&out).map_err(|_| ())?;
    Ok(s.to_string())
}

/// Collect all regular files under `root` recursively, skipping symlinks.
fn collect_files(root: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            continue; // Skip all symlinks
        }
        if meta.is_dir() {
            collect_files(&path, base, out)?;
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .to_string();
            out.push((rel, path));
        }
    }
    Ok(())
}

/// Chunked transfer encoding writer with a pre-allocated frame per flush.
struct ChunkedWriter<'a> {
    stream: &'a mut TcpStream,
    buf: Vec<u8>,
    chunk_size: usize,
    frame: Vec<u8>, // pre-allocated for single write_all per flush
}

impl<'a> ChunkedWriter<'a> {
    fn new(stream: &'a mut TcpStream, chunk_size: usize) -> Self {
        Self {
            stream,
            buf: Vec::with_capacity(chunk_size),
            chunk_size,
            frame: Vec::with_capacity(chunk_size + 32),
        }
    }

    fn write_chunk(&mut self, data: &[u8]) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let mut remaining = data;
        while !remaining.is_empty() {
            let space = self.chunk_size - self.buf.len();
            let take = space.min(remaining.len());
            self.buf.extend_from_slice(&remaining[..take]);
            remaining = &remaining[take..];
            if self.buf.len() == self.chunk_size {
                self.flush_chunk()?;
            }
        }
        Ok(())
    }

    fn flush_chunk(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk_len = self.buf.len();
        self.frame.clear();
        // Write chunk size in hex + CRLF
        write!(self.frame, "{:x}\r\n", chunk_len)?;
        // Append chunk data
        self.frame.extend_from_slice(&self.buf);
        // Append CRLF
        self.frame.extend_from_slice(b"\r\n");
        // Keep each chunk in one contiguous write_all call.
        self.stream.write_all(&self.frame)?;
        self.buf.clear();
        Ok(())
    }

    fn finish(mut self) -> io::Result<()> {
        self.flush_chunk()?;
        self.stream.write_all(b"0\r\n\r\n")?;
        Ok(())
    }
}

impl<'a> Write for ChunkedWriter<'a> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.write_chunk(data)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_chunk()
    }
}

struct ActivityWriter<'a, 's> {
    writer: &'a mut ChunkedWriter<'s>,
    state: &'a Arc<Mutex<SessionState>>,
}

impl Write for ActivityWriter<'_, '_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        touch_activity(self.state);
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        touch_activity(self.state);
        self.writer.flush()
    }
}

fn serve_download(
    stream: &mut TcpStream,
    path: &str,
    state: &Arc<Mutex<SessionState>>,
    token: &str,
    header_str: &str,
) -> io::Result<()> {
    touch_activity(state);
    let prefix = format!("/{}/download/", token);
    let rest = match path.strip_prefix(&prefix) {
        Some(r) => r,
        None => return respond(stream, 404, "text/plain", b"Not Found"),
    };

    // Parse <id>/<name> where <name> may contain subdirectories
    let (id_str, name_encoded) = match rest.split_once('/') {
        Some(p) => p,
        None => return respond(stream, 404, "text/plain", b"Not Found"),
    };
    let id: usize = match id_str.parse() {
        Ok(i) => i,
        Err(_) => return respond(stream, 404, "text/plain", b"Not Found"),
    };

    // Percent-decode name (using new function that returns Result)
    let decoded_name = match percent_decode(name_encoded) {
        Ok(d) => d,
        Err(()) => {
            return respond(
                stream,
                400,
                "text/plain",
                b"Bad Request: Invalid path encoding",
            )
        }
    };

    // Reject path traversal, leading slash, backslash
    if decoded_name.contains("..") || decoded_name.starts_with('/') || decoded_name.contains('\\') {
        return respond(stream, 403, "text/plain", b"Forbidden");
    }

    // Snapshot shared_paths under lock
    let root_path = {
        let st = lock_state(state);
        st.shared_paths.get(id).cloned()
    };
    let root_path = match root_path {
        Some(p) => p,
        None => return respond(stream, 404, "text/plain", b"Not Found"),
    };

    // Symlink & root-containment checks
    let root_meta = match std::fs::symlink_metadata(&root_path) {
        Ok(m) => m,
        Err(_) => return respond(stream, 404, "text/plain", b"Not Found"),
    };

    // If root_path itself is a symlink, verify its target stays inside parent
    if root_meta.file_type().is_symlink() {
        let canon_root = match root_path.canonicalize() {
            Ok(p) => p,
            Err(_) => return respond(stream, 403, "text/plain", b"Forbidden"),
        };
        let parent = root_path.parent().and_then(|p| p.canonicalize().ok());
        if let Some(parent_canon) = parent {
            if !canon_root.starts_with(&parent_canon) {
                return respond(stream, 403, "text/plain", b"Forbidden");
            }
        }
    }

    let (target_path, file_size) = if root_meta.is_dir() {
        // Directory: join decoded_name, check candidate
        let candidate = root_path.join(decoded_name);
        let cand_meta = match std::fs::symlink_metadata(&candidate) {
            Ok(m) => m,
            Err(_) => return respond(stream, 404, "text/plain", b"Not Found"),
        };
        if cand_meta.file_type().is_symlink() {
            return respond(stream, 403, "text/plain", b"Forbidden");
        }
        if !cand_meta.is_file() {
            return respond(stream, 404, "text/plain", b"Not Found");
        }
        let canon_root = match root_path.canonicalize() {
            Ok(p) => p,
            Err(_) => return respond(stream, 403, "text/plain", b"Forbidden"),
        };
        let canon_target = match candidate.canonicalize() {
            Ok(p) => p,
            Err(_) => return respond(stream, 403, "text/plain", b"Forbidden"),
        };
        if !canon_target.starts_with(&canon_root) {
            return respond(stream, 403, "text/plain", b"Forbidden");
        }
        (canon_target, cand_meta.len())
    } else if root_meta.is_file() {
        // Regular file: decoded_name must match either the registered st.files entry
        // (which preserves relative subdirectory paths) OR the bare file_name.
        let registered_name = {
            let st = lock_state(state);
            st.files.get(id).cloned()
        };
        let file_basename = root_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let expected_rel = registered_name.as_deref().unwrap_or(file_basename.as_str());
        if decoded_name != expected_rel && decoded_name != file_basename {
            return respond(stream, 404, "text/plain", b"Not Found");
        }
        let canon_root = match root_path.canonicalize() {
            Ok(p) => p,
            Err(_) => return respond(stream, 403, "text/plain", b"Forbidden"),
        };
        (canon_root, root_meta.len())
    } else {
        return respond(stream, 404, "text/plain", b"Not Found");
    };

    // Parse Range header from header_str
    let range_header = header_str
        .lines()
        .find(|l| l.to_lowercase().starts_with("range:"))
        .map(|l| l["range:".len()..].trim().to_string());

    // Open file
    let mut file = match std::fs::File::open(&target_path) {
        Ok(f) => f,
        Err(_) => return respond(stream, 404, "text/plain", b"Not Found"),
    };

    const CHUNK_SIZE: usize = 64 * 1024;
    let mut buf = vec![0u8; CHUNK_SIZE];

    if let Some(range) = range_header {
        // Parse Range: bytes=start-end, bytes=start-, bytes=-suffix
        if !range.starts_with("bytes=") {
            return respond(
                stream,
                400,
                "text/plain",
                b"Bad Request: Invalid Range unit",
            );
        }
        let range_spec = &range[6..];
        let (start, end) = if range_spec.starts_with('-') {
            // suffix-byte-range-spec: bytes=-suffix
            let suffix: u64 = match range_spec[1..].parse() {
                Ok(s) => s,
                Err(_) => {
                    return respond(
                        stream,
                        400,
                        "text/plain",
                        b"Bad Request: Invalid Range suffix",
                    )
                }
            };
            if file_size == 0 || suffix == 0 {
                // Guard: any Range on 0-byte file returns 416
                let cr = format!("bytes */{}", file_size);
                return respond_headers(
                    stream,
                    416,
                    &[
                        ("Content-Type", "text/plain"),
                        ("Content-Range", &cr),
                        ("Accept-Ranges", "bytes"),
                        ("Connection", "close"),
                    ],
                    Some(b"Range Not Satisfiable"),
                );
            }
            let start = file_size.saturating_sub(suffix);
            (start, file_size - 1)
        } else if range_spec.ends_with('-') {
            // bytes=start-
            let start: u64 = match range_spec[..range_spec.len() - 1].parse() {
                Ok(s) => s,
                Err(_) => {
                    return respond(
                        stream,
                        400,
                        "text/plain",
                        b"Bad Request: Invalid Range start",
                    )
                }
            };
            if start >= file_size {
                let cr = format!("bytes */{}", file_size);
                return respond_headers(
                    stream,
                    416,
                    &[
                        ("Content-Type", "text/plain"),
                        ("Content-Range", &cr),
                        ("Accept-Ranges", "bytes"),
                        ("Connection", "close"),
                    ],
                    Some(b"Range Not Satisfiable"),
                );
            }
            (start, file_size - 1)
        } else {
            // bytes=start-end
            let parts: Vec<&str> = range_spec.split('-').collect();
            if parts.len() != 2 {
                return respond(
                    stream,
                    400,
                    "text/plain",
                    b"Bad Request: Invalid Range format",
                );
            }
            let start: u64 = match parts[0].parse() {
                Ok(s) => s,
                Err(_) => {
                    return respond(
                        stream,
                        400,
                        "text/plain",
                        b"Bad Request: Invalid Range start",
                    )
                }
            };
            let end: u64 = match parts[1].parse() {
                Ok(e) => e,
                Err(_) => {
                    return respond(stream, 400, "text/plain", b"Bad Request: Invalid Range end")
                }
            };
            if start > end || start >= file_size {
                let cr = format!("bytes */{}", file_size);
                return respond_headers(
                    stream,
                    416,
                    &[
                        ("Content-Type", "text/plain"),
                        ("Content-Range", &cr),
                        ("Accept-Ranges", "bytes"),
                        ("Connection", "close"),
                    ],
                    Some(b"Range Not Satisfiable"),
                );
            }
            let end = end.min(file_size - 1);
            (start, end)
        };

        // Valid range: send 206 Partial Content
        let content_length = end - start + 1;
        let cr = format!("bytes {}-{}/{}", start, end, file_size);
        respond_headers(
            stream,
            206,
            &[
                ("Content-Type", "application/octet-stream"),
                ("Content-Range", &cr),
                ("Accept-Ranges", "bytes"),
                ("Content-Length", &content_length.to_string()),
                ("Connection", "close"),
            ],
            None,
        )?;

        // Seek to start
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(start))?;

        // Stream range
        let mut remaining = content_length;
        while remaining > 0 {
            touch_activity(state);
            let to_read = (CHUNK_SIZE as u64).min(remaining) as usize;
            let n = file.read(&mut buf[..to_read])?;
            if n == 0 {
                break;
            }
            stream.write_all(&buf[..n])?;
            remaining -= n as u64;
        }
    } else {
        // No Range header: full download
        respond_headers(
            stream,
            200,
            &[
                ("Content-Type", "application/octet-stream"),
                ("Accept-Ranges", "bytes"),
                ("Content-Length", &file_size.to_string()),
                ("Connection", "close"),
            ],
            None,
        )?;

        // Stream entire file
        loop {
            touch_activity(state);
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            stream.write_all(&buf[..n])?;
        }
    }

    // Update downloads_completed and check once/max_downloads
    let mut st = lock_state(state);
    st.downloads_completed += 1;
    if st.once
        || st
            .max_downloads
            .map_or(false, |max| st.downloads_completed >= max)
    {
        st.active = false;
    }

    Ok(())
}

fn serve_download_all(stream: &mut TcpStream, state: &Arc<Mutex<SessionState>>) -> io::Result<()> {
    touch_activity(state);
    // Snapshot zip_roots (or shared_paths fallback) under lock
    let shared_paths = {
        let st = lock_state(state);
        if st.zip_roots.is_empty() {
            st.shared_paths.clone()
        } else {
            st.zip_roots.clone()
        }
    };

    // Collect all regular files, skipping symlinks
    let mut entries = Vec::new();
    for root in &shared_paths {
        if root.is_dir() {
            collect_files(root, root, &mut entries)?;
        } else if root.is_file() {
            let name = root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();
            entries.push((name, root.clone()));
        }
    }

    if entries.is_empty() {
        return respond(
            stream,
            404,
            "text/plain",
            b"Not Found: No files to download",
        );
    }

    // Write HTTP 200 headers with chunked encoding
    let headers = [
        ("Content-Type", "application/zip"),
        (
            "Content-Disposition",
            "attachment; filename=\"lanbeam-files.zip\"",
        ),
        ("Transfer-Encoding", "chunked"),
        ("Connection", "close"),
    ];
    respond_headers(stream, 200, &headers, None)?;

    // Stream ZIP via ChunkedWriter
    let mut chunked = ChunkedWriter::new(stream, 65536);
    let mut activity_writer = ActivityWriter {
        writer: &mut chunked,
        state,
    };
    let _count = crate::zip::stream_zip_entries(&mut activity_writer, &entries)?;
    drop(activity_writer);
    chunked.finish()?;

    // Update downloads_completed and check once/max_downloads
    let mut st = lock_state(state);
    st.downloads_completed += 1;
    if st.once
        || st
            .max_downloads
            .map_or(false, |max| st.downloads_completed >= max)
    {
        st.active = false;
    }

    Ok(())
}

fn serve_upload(
    stream: &mut TcpStream,
    state: &Arc<Mutex<SessionState>>,
    query: Option<&str>,
    initial_body: &[u8],
    headers_str: &str,
) -> io::Result<()> {
    touch_activity(state);
    let filename = query.unwrap_or("").split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "name" && !value.is_empty()).then_some(value)
    });
    let filename = match filename {
        Some(name) => name,
        None => return respond(stream, 400, "text/plain", b"Bad Request: Missing filename"),
    };
    let decoded = match percent_decode(filename) {
        Ok(name) => name,
        Err(()) => return respond(stream, 400, "text/plain", b"Bad Request: Invalid filename"),
    };
    let safe_name = decoded
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or("")
        .trim();
    if safe_name.is_empty() || safe_name == "." || safe_name == ".." || safe_name.len() > 255 {
        return respond(stream, 400, "text/plain", b"Bad Request: Invalid filename");
    }

    // Check for chunked transfer encoding first
    if has_transfer_encoding(headers_str)
        && (!is_chunked_transfer(headers_str) || get_content_length(headers_str).is_some())
    {
        return respond(
            stream,
            400,
            "text/plain",
            b"Bad Request: Unsupported or ambiguous Transfer-Encoding",
        );
    }
    if is_chunked_transfer(headers_str) {
        return serve_upload_chunked(stream, state, query, initial_body, headers_str, safe_name);
    }

    // Content-Length path
    let content_length = match get_content_length(headers_str) {
        Some(n) if n <= 10 * 1024 * 1024 * 1024 => n as u64,
        Some(_) => {
            return respond(
                stream,
                400,
                "text/plain",
                b"Bad Request: Content-Length too large",
            )
        }
        None => {
            return respond(
                stream,
                400,
                "text/plain",
                b"Bad Request: Missing Content-Length",
            )
        }
    };
    let dest_dir = lock_state(state).dest_dir.clone();
    if std::fs::create_dir_all(&dest_dir).is_err() {
        return respond(stream, 500, "text/plain", b"Internal Server Error");
    }

    // Reserve both names atomically per upload so independent filenames start at suffix 0
    // while concurrent collisions retry cleanly with _1, _2, ...
    let mut next_suffix: u64 = 0;
    let (final_name, final_path, part_path, mut file) = loop {
        let suffix = next_suffix;
        next_suffix += 1;
        let candidate = if suffix == 0 {
            safe_name.to_string()
        } else {
            match safe_name.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() => format!("{}_{}.{}", stem, suffix, ext),
                _ => format!("{}_{}", safe_name, suffix),
            }
        };
        let final_path = dest_dir.join(&candidate);
        let part_path = dest_dir.join(format!(".{}.lanbeam.part", candidate));
        let reservation = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&final_path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                return respond(
                    stream,
                    500,
                    "text/plain",
                    b"Internal Server Error: Failed to reserve upload name",
                )
            }
        };
        drop(reservation);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part_path)
        {
            Ok(file) => break (candidate, final_path, part_path, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let _ = std::fs::remove_file(&final_path);
                continue;
            }
            Err(_) => {
                let _ = std::fs::remove_file(&final_path);
                return respond(
                    stream,
                    500,
                    "text/plain",
                    b"Internal Server Error: Failed to create upload file",
                );
            }
        }
    };

    let fail_cleanup = |file: std::fs::File| {
        drop(file);
        let _ = std::fs::remove_file(&part_path);
        let _ = std::fs::remove_file(&final_path);
    };
    let initial = initial_body.len().min(content_length as usize);
    if file.write_all(&initial_body[..initial]).is_err() {
        fail_cleanup(file);
        return respond(
            stream,
            500,
            "text/plain",
            b"Internal Server Error: Write failed",
        );
    }
    let mut written = initial as u64;
    let mut buf = [0u8; 64 * 1024];
    while written < content_length {
        let limit = buf.len().min((content_length - written) as usize);
        match stream.read(&mut buf[..limit]) {
            Ok(0) => {
                fail_cleanup(file);
                return respond(stream, 400, "text/plain", b"Bad Request: Premature EOF");
            }
            Ok(n) => {
                touch_activity(state);
                if file.write_all(&buf[..n]).is_err() {
                    fail_cleanup(file);
                    return respond(
                        stream,
                        500,
                        "text/plain",
                        b"Internal Server Error: Write failed",
                    );
                }
                written += n as u64;
            }
            Err(e) => {
                fail_cleanup(file);
                let (status, message) = if e.kind() == io::ErrorKind::TimedOut
                    || e.kind() == io::ErrorKind::WouldBlock
                {
                    (400, b"Bad Request: Premature EOF" as &[u8])
                } else {
                    (500, b"Internal Server Error: Read failed" as &[u8])
                };
                return respond(stream, status, "text/plain", message);
            }
        }
    }
    if file.flush().is_err() {
        fail_cleanup(file);
        return respond(
            stream,
            500,
            "text/plain",
            b"Internal Server Error: Flush failed",
        );
    }
    drop(file);
    if std::fs::rename(&part_path, &final_path).is_err() {
        let _ = std::fs::remove_file(&part_path);
        let _ = std::fs::remove_file(&final_path);
        return respond(
            stream,
            500,
            "text/plain",
            b"Internal Server Error: Rename failed",
        );
    }
    {
        let mut st = lock_state(state);
        st.uploads_received += 1;
        st.files.push(final_name.clone());
        if st
            .max_uploads
            .map_or(false, |max| st.uploads_received >= max)
        {
            st.active = false;
        }
    }
    #[cfg(not(test))]
    {
        let _ = std::process::Command::new("notify-send")
            .args(["lanbeam", &format!("Received: {}", final_name)])
            .status();
    }
    let body = format!(
        "{{\"ok\":true,\"filename\":\"{}\"}}",
        escape_json(&final_name)
    );
    respond(stream, 200, "application/json", body.as_bytes())
}

/// Handle upload with chunked transfer encoding.
fn serve_upload_chunked(
    stream: &mut TcpStream,
    state: &Arc<Mutex<SessionState>>,
    _query: Option<&str>,
    initial_body: &[u8],
    _headers_str: &str,
    safe_name: &str,
) -> io::Result<()> {
    let dest_dir = lock_state(state).dest_dir.clone();
    if std::fs::create_dir_all(&dest_dir).is_err() {
        return respond(stream, 500, "text/plain", b"Internal Server Error");
    }

    // Reserve both names atomically per upload
    let mut next_suffix: u64 = 0;
    let (final_name, final_path, part_path, mut file) = loop {
        let suffix = next_suffix;
        next_suffix += 1;
        let candidate = if suffix == 0 {
            safe_name.to_string()
        } else {
            match safe_name.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() => format!("{}_{}.{}", stem, suffix, ext),
                _ => format!("{}_{}", safe_name, suffix),
            }
        };
        let final_path = dest_dir.join(&candidate);
        let part_path = dest_dir.join(format!(".{}.lanbeam.part", candidate));
        let reservation = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&final_path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                return respond(
                    stream,
                    500,
                    "text/plain",
                    b"Internal Server Error: Failed to reserve upload name",
                )
            }
        };
        drop(reservation);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part_path)
        {
            Ok(file) => break (candidate, final_path, part_path, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let _ = std::fs::remove_file(&final_path);
                continue;
            }
            Err(_) => {
                let _ = std::fs::remove_file(&final_path);
                return respond(
                    stream,
                    500,
                    "text/plain",
                    b"Internal Server Error: Failed to create upload file",
                );
            }
        }
    };

    let fail_cleanup = |file: std::fs::File| {
        drop(file);
        let _ = std::fs::remove_file(&part_path);
        let _ = std::fs::remove_file(&final_path);
    };

    // Use the shared chunked body reader with max 10 GiB
    let result = read_chunked_body(
        stream,
        initial_body,
        state,
        10 * 1024 * 1024 * 1024,
        |chunk| {
            touch_activity(state);
            file.write_all(chunk)
                .map_err(|_| (500, "Internal Server Error: Write failed"))
        },
    );

    match result {
        Ok(_) => {
            // Success
            if file.flush().is_err() {
                fail_cleanup(file);
                return respond(
                    stream,
                    500,
                    "text/plain",
                    b"Internal Server Error: Flush failed",
                );
            }
            drop(file);
            if std::fs::rename(&part_path, &final_path).is_err() {
                let _ = std::fs::remove_file(&part_path);
                let _ = std::fs::remove_file(&final_path);
                return respond(
                    stream,
                    500,
                    "text/plain",
                    b"Internal Server Error: Rename failed",
                );
            }
            {
                let mut st = lock_state(state);
                st.uploads_received += 1;
                st.files.push(final_name.clone());
                if st
                    .max_uploads
                    .map_or(false, |max| st.uploads_received >= max)
                {
                    st.active = false;
                }
            }
            #[cfg(not(test))]
            {
                let _ = std::process::Command::new("notify-send")
                    .args(["lanbeam", &format!("Received: {}", final_name)])
                    .status();
            }
            let body = format!(
                "{{\"ok\":true,\"filename\":\"{}\"}}",
                escape_json(&final_name)
            );
            respond(stream, 200, "application/json", body.as_bytes())
        }
        Err((status, message)) => {
            fail_cleanup(file);
            respond(stream, status, "text/plain", message.as_bytes())
        }
    }
}

fn extract_json_text_field(body: &str) -> Option<String> {
    let key_pos = body.find("\"text\"")?;
    let after_key = body[key_pos + "\"text\"".len()..].trim_start();
    let after_colon = after_key.strip_prefix(':')?.trim_start();
    let rest = after_colon.strip_prefix('"')?;

    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'b' => out.push('\u{0008}'),
                'f' => out.push('\u{000C}'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let mut hex = String::with_capacity(4);
                    for _ in 0..4 {
                        hex.push(chars.next()?);
                    }
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            out.push(ch);
                        }
                    }
                }
                other => out.push(other),
            },
            ch => out.push(ch),
        }
    }
    None
}

fn serve_text(
    stream: &mut TcpStream,
    state: &Arc<Mutex<SessionState>>,
    body: &str,
) -> io::Result<()> {
    touch_activity(state);
    let text = extract_json_text_field(body).unwrap_or_default();

    if !text.is_empty() {
        let mut st = lock_state(state);
        st.shared_text = Some(text.clone());
        drop(st);

        #[cfg(test)]
        {
            // In tests, don't spawn external processes
        }
        #[cfg(not(test))]
        {
            let _ = Command::new("wl-copy").arg(&text).status();
            let _ = Command::new("notify-send")
                .args(["lanbeam", &text])
                .status();
        }
    }

    respond(stream, 200, "application/json", b"{\"ok\":true}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::Duration;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), id))
    }

    fn start_test_server() -> (
        String,
        u16,
        thread::JoinHandle<()>,
        Arc<Mutex<SessionState>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "testtoken123";
        let session = ServerSession::new(listener, port, token).unwrap();
        let state_clone = session.state.clone();
        let handle = thread::spawn(move || {
            let _ = session.run();
        });
        // Wait a bit for server to start
        thread::sleep(Duration::from_millis(50));
        (token.to_string(), port, handle, state_clone)
    }

    fn read_response(stream: &mut std::net::TcpStream) -> (u16, Vec<u8>, String) {
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();

        // Parse status line
        let header_end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(0);
        let headers = String::from_utf8_lossy(&buf[..header_end]);
        let status_line = headers.lines().next().unwrap_or("");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);

        // Check for chunked encoding
        let is_chunked = headers.lines().any(|l| {
            l.to_lowercase().starts_with("transfer-encoding:")
                && l.to_lowercase().contains("chunked")
        });

        let content_range = headers
            .lines()
            .find(|l| l.to_lowercase().starts_with("content-range:"))
            .map(|l| l.to_string())
            .unwrap_or_default();

        let body = if is_chunked {
            // Decode chunked encoding
            let mut body = Vec::new();
            let mut pos = header_end + 4;
            while pos < buf.len() {
                // Find chunk size line
                let line_end = buf[pos..]
                    .windows(2)
                    .position(|w| w == b"\r\n")
                    .map(|p| p + pos);
                let line_end = match line_end {
                    Some(e) => e,
                    None => break,
                };
                let chunk_size_line = String::from_utf8_lossy(&buf[pos..line_end]);
                let chunk_size = usize::from_str_radix(chunk_size_line.trim(), 16).unwrap_or(0);
                if chunk_size == 0 {
                    break;
                }
                let data_start = line_end + 2;
                let data_end = data_start + chunk_size;
                if data_end > buf.len() {
                    break;
                }
                body.extend_from_slice(&buf[data_start..data_end]);
                pos = data_end + 2; // Skip \r\n after chunk data
            }
            body
        } else {
            if header_end + 4 < buf.len() {
                buf[header_end + 4..].to_vec()
            } else {
                Vec::new()
            }
        };
        (status, body, content_range)
    }

    fn http_get(port: u16, path: &str) -> (u16, Vec<u8>) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            path
        );
        stream.write_all(req.as_bytes()).unwrap();

        let (status, body, _) = read_response(&mut stream);
        (status, body)
    }

    fn http_get_with_range(port: u16, path: &str, range: &str) -> (u16, Vec<u8>, String) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: localhost\r\nRange: {}\r\nConnection: close\r\n\r\n",
            path, range
        );
        stream.write_all(req.as_bytes()).unwrap();

        let (status, body, content_range) = read_response(&mut stream);
        (status, body, content_range)
    }

    fn http_post_text(port: u16, token: &str, text: &str) -> (u16, Vec<u8>) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let body = format!("{{\"text\":\"{}\"}}", text);
        let req = format!(
            "POST /{}/text HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            token,
            body.len(),
            body
        );
        stream.write_all(req.as_bytes()).unwrap();

        let (status, body, _) = read_response(&mut stream);
        (status, body)
    }

    #[test]
    fn session_state_defaults() {
        let st = SessionState::default();
        assert!(st.active);
        assert_eq!(st.downloads_completed, 0);
        assert_eq!(st.shared_paths.len(), 0);
        assert!(!st.once);
        assert_eq!(st.max_downloads, None);
    }

    #[test]
    fn configure_send_sets_state() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = ServerSession::new(listener, port, "token123").unwrap();

        let paths = vec![
            std::path::PathBuf::from("/tmp/a.txt"),
            std::path::PathBuf::from("/tmp/b.txt"),
        ];
        session.configure_send(
            paths.clone(),
            true,
            Some(5),
            Some(30),
            "http://example.com".to_string(),
        );

        let st = lock_state(&session.state);
        assert_eq!(st.mode, "send");
        assert_eq!(st.files.len(), 2);
        assert_eq!(st.shared_paths.len(), 2);
        assert!(st.once);
        assert_eq!(st.max_downloads, Some(5));
        assert_eq!(st.timeout_secs, Some(30));
        assert_eq!(st.url, "http://example.com");
    }

    #[test]
    fn configure_receive_sets_state() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = ServerSession::new(listener, port, "token123").unwrap();

        session.configure_receive(
            std::path::PathBuf::from("/tmp/recv"),
            Some(10),
            Some(60),
            "http://example.com".to_string(),
        );

        let st = lock_state(&session.state);
        assert_eq!(st.mode, "receive");
        assert_eq!(st.dest_dir, std::path::PathBuf::from("/tmp/recv"));
        assert_eq!(st.max_uploads, Some(10));
        assert_eq!(st.timeout_secs, Some(60));
        assert_eq!(st.url, "http://example.com");
    }

    #[test]
    fn configure_text_sets_state() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = ServerSession::new(listener, port, "token123").unwrap();

        session.configure_text(
            Some("hello".to_string()),
            true,
            Some(30),
            "http://example.com".to_string(),
        );

        let st = lock_state(&session.state);
        assert_eq!(st.mode, "text");
        assert_eq!(st.shared_text, Some("hello".to_string()));
        assert!(st.once);
        assert_eq!(st.timeout_secs, Some(30));
        assert_eq!(st.url, "http://example.com");
    }

    #[test]
    fn token_routing_root_returns_403() {
        let (token, port, handle, _state) = start_test_server();
        let (status, _) = http_get(port, "/");
        assert_eq!(status, 403);
        let (status, _) = http_get(port, "/wrong");
        assert_eq!(status, 403);
        let (status, _) = http_get(port, "/wrong/status");
        assert_eq!(status, 403);
        let (status, _) = http_get(port, &format!("/{}extra", token));
        assert_eq!(status, 403);
        lock_state(&_state).active = false;
        handle.join().ok();
    }

    #[test]
    fn token_routing_valid_paths_allowed() {
        let (token, port, handle, _state) = start_test_server();
        let (status, _) = http_get(port, &format!("/{}", token));
        assert_eq!(status, 200);
        let (status, _) = http_get(port, &format!("/{}/", token));
        assert_eq!(status, 200);
        let (status, _) = http_get(port, &format!("/{}/status", token));
        assert_eq!(status, 200);
        let (status, _) = http_get(port, &format!("/{}/qr.svg", token));
        assert_eq!(status, 200);
        lock_state(&_state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_qr_encodes_session_url() {
        let (token, port, handle, state) = start_test_server();

        // Set session URL
        {
            let mut st = lock_state(&state);
            st.url = format!("http://192.168.1.5:5000/{}", token);
        }

        let (status, body) = http_get(port, &format!("/{}/qr.svg", token));
        assert_eq!(status, 200);
        let svg = String::from_utf8(body).unwrap();
        // QR is a visual encoding - verify it's valid SVG with QR structure
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("<rect"));
        assert!(svg.trim_end().ends_with("</svg>"));
        // The URL is encoded visually in the QR, not as plain text

        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_app_injects_initial_state() {
        let (token, port, handle, state) = start_test_server();

        {
            let mut st = lock_state(&state);
            st.url = format!("http://192.168.1.5:5000/{}", token);
            st.mode = "send".to_string();
            st.files = vec!["test.txt".to_string()];
            st.downloads_completed = 0;
            st.uploads_received = 0;
            st.shared_text = Some("hello</script>\nthere".to_string());
            st.remaining_seconds = 300;
            st.active = true;
        }

        let (status, body) = http_get(port, &format!("/{}", token));
        assert_eq!(status, 200);
        let html = String::from_utf8(body).unwrap();
        // Should contain the initial state injection
        assert!(html.contains("__LANBEAM_INITIAL_STATE__"));
        assert!(html.contains(&format!("\"token\":\"{}\"", token)));
        assert!(html.contains("send"));
        assert!(html.contains("test.txt"));
        assert!(html.contains("hello\\u003c/script>\\nthere"));
        assert!(html.contains("300"));
        assert!(html.contains("true"));

        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_download_full_file() {
        let (token, port, handle, state) = start_test_server();

        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file_path = tmp_dir.join("test.txt");
        std::fs::write(&file_path, b"Hello, world!").unwrap();

        {
            let mut st = lock_state(&state);
            st.shared_paths = vec![file_path.clone()];
            st.files = vec!["test.txt".to_string()];
            st.mode = "send".to_string();
        }

        let (status, body) = http_get(port, &format!("/{}/download/0/test.txt", token));
        assert_eq!(status, 200);
        assert_eq!(body, b"Hello, world!");

        {
            let st = lock_state(&state);
            assert_eq!(st.downloads_completed, 1);
        }
        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_download_range_partial_content() {
        let (token, port, handle, state) = start_test_server();

        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file_path = tmp_dir.join("test.txt");
        std::fs::write(&file_path, b"0123456789ABCDEF").unwrap(); // 16 bytes

        {
            let mut st = lock_state(&state);
            st.shared_paths = vec![file_path.clone()];
            st.files = vec!["test.txt".to_string()];
            st.mode = "send".to_string();
        }

        let (status, body, cr) = http_get_with_range(
            port,
            &format!("/{}/download/0/test.txt", token),
            "bytes=0-9",
        );
        assert_eq!(status, 206);
        assert_eq!(body, b"0123456789");
        assert!(cr.contains("bytes 0-9/16"));

        {
            let st = lock_state(&state);
            assert_eq!(st.downloads_completed, 1);
        }
        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_download_range_suffix() {
        let (token, port, handle, state) = start_test_server();

        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file_path = tmp_dir.join("test.txt");
        std::fs::write(&file_path, b"0123456789ABCDEF").unwrap(); // 16 bytes

        {
            let mut st = lock_state(&state);
            st.shared_paths = vec![file_path.clone()];
            st.files = vec!["test.txt".to_string()];
            st.mode = "send".to_string();
        }

        let (status, body, cr) =
            http_get_with_range(port, &format!("/{}/download/0/test.txt", token), "bytes=-6");
        assert_eq!(status, 206);
        assert_eq!(body, b"ABCDEF");
        assert!(cr.contains("bytes 10-15/16"));

        {
            let st = lock_state(&state);
            assert_eq!(st.downloads_completed, 1);
        }
        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_download_empty_file_range_416() {
        let (token, port, handle, state) = start_test_server();

        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file_path = tmp_dir.join("empty.txt");
        std::fs::write(&file_path, b"").unwrap();

        {
            let mut st = lock_state(&state);
            st.shared_paths = vec![file_path.clone()];
            st.files = vec!["empty.txt".to_string()];
            st.mode = "send".to_string();
        }

        let (status, _, cr) = http_get_with_range(
            port,
            &format!("/{}/download/0/empty.txt", token),
            "bytes=0-0",
        );
        assert_eq!(status, 416);
        assert!(cr.contains("bytes */0"));

        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_download_invalid_range_416() {
        let (token, port, handle, state) = start_test_server();

        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file_path = tmp_dir.join("test.txt");
        std::fs::write(&file_path, b"123").unwrap(); // 3 bytes

        {
            let mut st = lock_state(&state);
            st.shared_paths = vec![file_path.clone()];
            st.files = vec!["test.txt".to_string()];
            st.mode = "send".to_string();
        }

        let (status, _, cr) = http_get_with_range(
            port,
            &format!("/{}/download/0/test.txt", token),
            "bytes=5-10",
        );
        assert_eq!(status, 416);
        assert!(cr.contains("bytes */3"));

        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_download_subdirectory() {
        let (token, port, handle, state) = start_test_server();

        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let sub_dir = tmp_dir.join("sub");
        std::fs::create_dir(&sub_dir).unwrap();
        let file_path = sub_dir.join("file.txt");
        std::fs::write(&file_path, b"sub file").unwrap();

        {
            let mut st = lock_state(&state);
            st.shared_paths = vec![tmp_dir.clone()];
            st.files = vec!["sub/file.txt".to_string()];
            st.mode = "send".to_string();
        }

        let (status, body) = http_get(port, &format!("/{}/download/0/sub/file.txt", token));
        assert_eq!(status, 200);
        assert_eq!(body, b"sub file");

        {
            let st = lock_state(&state);
            assert_eq!(st.downloads_completed, 1);
        }
        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_download_symlink_escape_403() {
        let (token, port, handle, state) = start_test_server();

        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        // Create a file outside the shared directory
        let outside_dir = unique_temp_dir("lanbeam_outside");
        std::fs::create_dir_all(&outside_dir).unwrap();
        let _outside_guard = TempDirGuard(outside_dir.clone());
        let secret_file = outside_dir.join("secret.txt");
        std::fs::write(&secret_file, b"secret").unwrap();

        // Create symlink inside shared dir pointing outside
        let link_path = tmp_dir.join("escape.txt");
        symlink(&secret_file, &link_path).unwrap();

        {
            let mut st = lock_state(&state);
            st.shared_paths = vec![tmp_dir.clone()];
            st.files = vec!["escape.txt".to_string()];
            st.mode = "send".to_string();
        }

        let (status, _) = http_get(port, &format!("/{}/download/0/escape.txt", token));
        assert_eq!(status, 403);

        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_download_once_triggers_shutdown() {
        let (token, port, handle, state) = start_test_server();

        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file_path = tmp_dir.join("test.txt");
        std::fs::write(&file_path, b"test").unwrap();

        {
            let mut st = lock_state(&state);
            st.shared_paths = vec![file_path.clone()];
            st.files = vec!["test.txt".to_string()];
            st.mode = "send".to_string();
            st.once = true;
        }

        let (status, _) = http_get(port, &format!("/{}/download/0/test.txt", token));
        assert_eq!(status, 200);

        {
            let st = lock_state(&state);
            assert!(!st.active);
        }
        handle.join().ok();
    }

    #[test]
    fn serve_download_max_downloads_triggers_shutdown() {
        let (token, port, handle, state) = start_test_server();

        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file_path = tmp_dir.join("test.txt");
        std::fs::write(&file_path, b"test").unwrap();

        {
            let mut st = lock_state(&state);
            st.shared_paths = vec![file_path.clone()];
            st.files = vec!["test.txt".to_string()];
            st.mode = "send".to_string();
            st.max_downloads = Some(2);
        }

        // First download
        let (status, _) = http_get(port, &format!("/{}/download/0/test.txt", token));
        assert_eq!(status, 200);
        {
            let st = lock_state(&state);
            assert!(st.active);
        }

        // Second download
        let (status, _) = http_get(port, &format!("/{}/download/0/test.txt", token));
        assert_eq!(status, 200);
        {
            let st = lock_state(&state);
            assert!(!st.active);
            assert_eq!(st.downloads_completed, 2);
        }
        handle.join().ok();
    }

    #[test]
    fn serve_download_all_streams_zip() {
        let (token, port, handle, state) = start_test_server();

        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file1 = tmp_dir.join("a.txt");
        std::fs::write(&file1, b"content a").unwrap();

        let file2 = tmp_dir.join("b.txt");
        std::fs::write(&file2, b"content b").unwrap();

        // Add a symlink (should be skipped)
        let link_path = tmp_dir.join("link.txt");
        symlink(&file1, &link_path).unwrap();

        {
            let mut st = lock_state(&state);
            st.shared_paths = vec![tmp_dir.clone()];
            st.files = vec!["a.txt".to_string(), "b.txt".to_string()];
            st.mode = "send".to_string();
        }

        let (status, body) = http_get(port, &format!("/{}/download-all.zip", token));
        assert_eq!(status, 200);
        assert!(body.len() > 0);
        // Check it's a valid ZIP
        assert!(body.starts_with(&crate::zip::ZIP_LOCAL_HEADER_SIG.to_le_bytes()));
        let eocd_start = body.len() - 22;
        assert!(body[eocd_start..].starts_with(&crate::zip::ZIP_END_CENTRAL_DIR_SIG.to_le_bytes()));
        // Should have 2 entries (symlink skipped)
        let eocd = &body[eocd_start..];
        let total_entries = u16::from_le_bytes([eocd[10], eocd[11]]);
        assert_eq!(total_entries, 2);

        {
            let st = lock_state(&state);
            assert_eq!(st.downloads_completed, 1);
        }
        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn percent_decode_valid() {
        assert_eq!(
            percent_decode("hello%20world"),
            Ok("hello world".to_string())
        );
        assert_eq!(percent_decode("file%2Etxt"), Ok("file.txt".to_string()));
        assert_eq!(
            percent_decode("path%2Fto%2Ffile"),
            Ok("path/to/file".to_string())
        );
        // Valid UTF-8 multi-byte: %C3%A9 = é
        assert_eq!(percent_decode("%C3%A9"), Ok("é".to_string()));
        // Raw UTF-8 should pass through
        assert_eq!(percent_decode("café"), Ok("café".to_string()));
    }

    #[test]
    fn percent_decode_invalid() {
        assert!(percent_decode("hello%2").is_err()); // incomplete
        assert!(percent_decode("hello%GG").is_err()); // invalid hex
        assert!(percent_decode("hello%00world").is_err()); // null byte
        assert!(percent_decode("hello%0Aworld").is_err()); // control char
        assert!(percent_decode("path\\to\\file").is_err()); // backslash
                                                            // Invalid UTF-8 sequence: %FF is not valid UTF-8 start
        assert!(percent_decode("%FF").is_err());
        // Overlong encoding
        assert!(percent_decode("%C0%80").is_err());
    }

    #[test]
    fn serve_text_missing_content_length_400() {
        let (token, port, handle, _state) = start_test_server();

        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let body = "{\"text\":\"hello\"}";
        let req = format!(
            "POST /{}/text HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
            token, body
        );
        stream.write_all(req.as_bytes()).unwrap();

        let (status, body, _) = read_response(&mut stream);
        assert_eq!(status, 400);
        assert!(String::from_utf8_lossy(&body).contains("Missing Content-Length"));

        lock_state(&_state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_text_oversized_content_length_400() {
        let (token, port, handle, _state) = start_test_server();

        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let oversized_len = 65 * 1024; // 64 KiB + 1 KiB
                                       // Send only headers with oversized Content-Length, no body
        let req = format!(
            "POST /{}/text HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            token, oversized_len
        );
        stream.write_all(req.as_bytes()).unwrap();

        let (status, body, _) = read_response(&mut stream);
        assert_eq!(status, 400);
        assert!(String::from_utf8_lossy(&body).contains("exceeds 64 KiB"));

        lock_state(&_state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_text_premature_eof_400() {
        let (token, port, handle, _state) = start_test_server();

        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let body = "{\"text\":\"hello\"}";
        // Claim Content-Length: 100 but only send shorter body
        let req = format!(
            "POST /{}/text HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{}",
            token, body
        );
        stream.write_all(req.as_bytes()).unwrap();
        // Shutdown write so server sees EOF on read
        stream.shutdown(std::net::Shutdown::Write).unwrap();

        let (status, body, _) = read_response(&mut stream);
        assert_eq!(status, 400);
        assert!(String::from_utf8_lossy(&body).contains("Premature EOF"));

        lock_state(&_state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_text_invalid_utf8_400() {
        let (token, port, handle, _state) = start_test_server();

        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        // Send invalid UTF-8: 0xFF is not valid UTF-8
        let body = "{\"text\":\"";
        let invalid_utf8 = [0xFFu8];
        let req = format!(
            "POST /{}/text HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            token,
            body.len() + invalid_utf8.len() + 1, // +1 for closing quote
            body
        );
        stream.write_all(req.as_bytes()).unwrap();
        stream.write_all(&invalid_utf8).unwrap();
        stream.write_all(b"\"}").unwrap();

        let (status, body, _) = read_response(&mut stream);
        assert_eq!(status, 400);
        assert!(String::from_utf8_lossy(&body).contains("Invalid UTF-8"));

        lock_state(&_state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_text_success() {
        let (token, port, handle, state) = start_test_server();

        let (status, body) = http_post_text(port, &token, "hello world");
        assert_eq!(status, 200);
        assert!(String::from_utf8_lossy(&body).contains("ok"));

        {
            let st = lock_state(&state);
            assert_eq!(st.shared_text, Some("hello world".to_string()));
        }
        // Drop lock before setting active = false
        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn concurrent_parallel_uploads_deduplicate_cleanly() {
        use std::collections::HashSet;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "testparallelupload";
        let session = ServerSession::new(listener, port, token).unwrap();
        let dest_dir = unique_temp_dir("parallel_uploads");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let _guard = TempDirGuard(dest_dir.clone());
        session.configure_receive(
            dest_dir.clone(),
            None,
            None,
            format!("http://127.0.0.1:{}/{}", port, token),
        );
        let handle = thread::spawn(move || {
            let _ = session.run();
        });
        thread::sleep(Duration::from_millis(50));

        let payloads: Vec<Vec<u8>> = (0..6)
            .map(|i| format!("parallel payload {i}").into_bytes())
            .collect();
        let joins: Vec<_> = payloads
            .iter()
            .cloned()
            .map(|payload| {
                thread::spawn(move || {
                    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let headers = format!(
                        "POST /{}/upload?name=same.txt HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        token,
                        payload.len()
                    );
                    stream.write_all(headers.as_bytes()).unwrap();
                    stream.write_all(&payload).unwrap();
                    let (status, _, _) = read_response(&mut stream);
                    status
                })
            })
            .collect();
        for join in joins {
            assert_eq!(join.join().unwrap(), 200);
        }

        let mut actual_names: HashSet<String> = HashSet::new();
        let mut actual_payloads: HashSet<Vec<u8>> = HashSet::new();
        for entry in std::fs::read_dir(&dest_dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(
                !name.ends_with(".lanbeam.part"),
                "temp part file left behind: {name}"
            );
            actual_names.insert(name);
            actual_payloads.insert(std::fs::read(entry.path()).unwrap());
        }
        let expected_names: HashSet<String> = [
            "same.txt",
            "same_1.txt",
            "same_2.txt",
            "same_3.txt",
            "same_4.txt",
            "same_5.txt",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(actual_names, expected_names);
        let expected: HashSet<Vec<u8>> = payloads.into_iter().collect();
        assert_eq!(
            actual_payloads, expected,
            "all payloads must survive regardless of upload completion order"
        );

        // Stop through the authenticated endpoint so the listener thread is joined.
        let mut stop = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let req = format!(
            "POST /{}/stop HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            token
        );
        stop.write_all(req.as_bytes()).unwrap();
        let _ = read_response(&mut stop);
        handle.join().unwrap();
    }

    fn http_post_upload(port: u16, token: &str, name: &str, payload: &[u8]) -> (u16, Vec<u8>) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let req = format!(
            "POST /{}/upload?name={} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            token,
            name,
            payload.len()
        );
        stream.write_all(req.as_bytes()).unwrap();
        stream.write_all(payload).unwrap();
        let (status, body, _) = read_response(&mut stream);
        (status, body)
    }

    #[test]
    fn upload_integration_and_independent_filenames() {
        let (token, port, handle, state) = start_test_server();
        let dest_dir = unique_temp_dir("upload_integ");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let _guard = TempDirGuard(dest_dir.clone());
        {
            let mut st = lock_state(&state);
            st.mode = "receive".to_string();
            st.dest_dir = dest_dir.clone();
        }

        // First file a.txt -> a.txt
        let (s1, b1) = http_post_upload(port, &token, "a.txt", b"alpha");
        assert_eq!(s1, 200);
        assert!(String::from_utf8_lossy(&b1).contains("\"filename\":\"a.txt\""));

        // Different file b.txt must land as b.txt (NOT b_1.txt)
        let (s2, b2) = http_post_upload(port, &token, "b.txt", b"beta");
        assert_eq!(s2, 200);
        assert!(String::from_utf8_lossy(&b2).contains("\"filename\":\"b.txt\""));

        // Duplicate a.txt must deduplicate to a_1.txt
        let (s3, b3) = http_post_upload(port, &token, "a.txt", b"alpha2");
        assert_eq!(s3, 200);
        assert!(String::from_utf8_lossy(&b3).contains("\"filename\":\"a_1.txt\""));

        assert_eq!(std::fs::read(dest_dir.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(std::fs::read(dest_dir.join("b.txt")).unwrap(), b"beta");
        assert_eq!(std::fs::read(dest_dir.join("a_1.txt")).unwrap(), b"alpha2");

        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn upload_rejects_invalid_names_oversized_and_premature_eof() {
        let (token, port, handle, state) = start_test_server();
        let dest_dir = unique_temp_dir("upload_reject");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let _guard = TempDirGuard(dest_dir.clone());
        {
            let mut st = lock_state(&state);
            st.mode = "receive".to_string();
            st.dest_dir = dest_dir.clone();
        }

        // Empty / traversal name -> 400
        let (s_dotdot, _) = http_post_upload(port, &token, "..", b"x");
        assert_eq!(s_dotdot, 400);
        let (s_ctrl, _) = http_post_upload(port, &token, "bad%0A.txt", b"x");
        assert_eq!(s_ctrl, 400);

        // Oversized (> 10 GiB) headers-only -> 400
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let huge = format!(
            "POST /{}/upload?name=huge.bin HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10737418241\r\nConnection: close\r\n\r\n",
            token
        );
        stream.write_all(huge.as_bytes()).unwrap();
        let (s_huge, _, _) = read_response(&mut stream);
        assert_eq!(s_huge, 400);

        // Premature EOF -> 400 and zero leftover files
        let mut eof_stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        eof_stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let partial = format!(
            "POST /{}/upload?name=cut.bin HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort",
            token
        );
        eof_stream.write_all(partial.as_bytes()).unwrap();
        eof_stream.shutdown(std::net::Shutdown::Write).unwrap();
        let (s_eof, _, _) = read_response(&mut eof_stream);
        assert_eq!(s_eof, 400);
        assert_eq!(std::fs::read_dir(&dest_dir).unwrap().count(), 0);

        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn upload_max_uploads_deactivates_and_stop_endpoint_works() {
        let (token, port, handle, state) = start_test_server();
        let dest_dir = unique_temp_dir("upload_max");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let _guard = TempDirGuard(dest_dir.clone());
        {
            let mut st = lock_state(&state);
            st.mode = "receive".to_string();
            st.dest_dir = dest_dir;
            st.max_uploads = Some(2);
        }

        let (s1, _) = http_post_upload(port, &token, "one.txt", b"1");
        assert_eq!(s1, 200);
        assert!(lock_state(&state).active);

        let (s2, _) = http_post_upload(port, &token, "two.txt", b"2");
        assert_eq!(s2, 200);
        assert!(!lock_state(&state).active);
        handle.join().unwrap();

        // Verify POST /<token>/stop also shuts down an active session cleanly
        let (token2, port2, handle2, state2) = start_test_server();
        let mut stop = std::net::TcpStream::connect(("127.0.0.1", port2)).unwrap();
        stop.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let req = format!(
            "POST /{}/stop HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            token2
        );
        stop.write_all(req.as_bytes()).unwrap();
        let (s_stop, b_stop, _) = read_response(&mut stop);
        assert_eq!(s_stop, 200);
        assert_eq!(b_stop, b"Stopping");
        assert!(!lock_state(&state2).active);
        handle2.join().unwrap();
    }

    #[test]
    fn configure_send_expands_directories_and_preserves_zip_roots() {
        let root_dir = unique_temp_dir("send_expand");
        let sub_dir = root_dir.join("nested");
        std::fs::create_dir_all(&sub_dir).unwrap();
        let _guard = TempDirGuard(root_dir.clone());

        std::fs::write(root_dir.join("first.txt"), b"first").unwrap();
        std::fs::write(sub_dir.join("second.txt"), b"second").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "testdirexpand";
        let session = ServerSession::new(listener, port, token).unwrap();
        session.configure_send(
            vec![root_dir.clone()],
            false,
            None,
            None,
            format!("http://127.0.0.1:{}/{}", port, token),
        );

        let state = session.state.clone();
        let handle = thread::spawn(move || {
            let _ = session.run();
        });
        thread::sleep(Duration::from_millis(50));

        {
            let st = lock_state(&state);
            assert_eq!(st.files, vec!["first.txt", "nested/second.txt"]);
            assert_eq!(st.zip_roots, vec![root_dir]);
        }

        // Both should be downloadable with their relative paths
        let (s0, b0) = http_get(port, &format!("/{}/download/0/first.txt", token));
        assert_eq!(s0, 200);
        assert_eq!(b0, b"first");

        let (s1, b1) = http_get(port, &format!("/{}/download/1/nested/second.txt", token));
        assert_eq!(s1, 200);
        assert_eq!(b1, b"second");

        // Downloading by basename should also work (unique index)
        let (s_base1, b_base1) = http_get(port, &format!("/{}/download/1/second.txt", token));
        assert_eq!(s_base1, 200);
        assert_eq!(b_base1, b"second");

        let (szip, zip_bytes) = http_get(port, &format!("/{}/download-all.zip", token));
        assert_eq!(szip, 200);
        assert!(String::from_utf8_lossy(&zip_bytes).contains("nested/second.txt"));

        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn configure_send_duplicate_basenames_in_subdirs() {
        let root_dir = unique_temp_dir("send_dup");
        let sub_a = root_dir.join("a");
        let sub_b = root_dir.join("b");
        std::fs::create_dir_all(&sub_a).unwrap();
        std::fs::create_dir_all(&sub_b).unwrap();
        let _guard = TempDirGuard(root_dir.clone());

        std::fs::write(sub_a.join("dup.txt"), b"from a").unwrap();
        std::fs::write(sub_b.join("dup.txt"), b"from b").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "testdupnames";
        let session = ServerSession::new(listener, port, token).unwrap();
        session.configure_send(
            vec![root_dir.clone()],
            false,
            None,
            None,
            format!("http://127.0.0.1:{}/{}", port, token),
        );

        let state = session.state.clone();
        let handle = thread::spawn(move || {
            let _ = session.run();
        });
        thread::sleep(Duration::from_millis(50));

        {
            let st = lock_state(&state);
            assert_eq!(st.files, vec!["a/dup.txt", "b/dup.txt"]);
            assert_eq!(st.zip_roots, vec![root_dir]);
        }

        // Both should be downloadable with their relative paths
        let (s0, b0) = http_get(port, &format!("/{}/download/0/a/dup.txt", token));
        assert_eq!(s0, 200);
        assert_eq!(b0, b"from a");

        let (s1, b1) = http_get(port, &format!("/{}/download/1/b/dup.txt", token));
        assert_eq!(s1, 200);
        assert_eq!(b1, b"from b");

        // Downloading by basename works because index uniquely selects the file
        let (s_base0, b_base0) = http_get(port, &format!("/{}/download/0/dup.txt", token));
        assert_eq!(s_base0, 200);
        assert_eq!(b_base0, b"from a");

        let (s_base1, b_base1) = http_get(port, &format!("/{}/download/1/dup.txt", token));
        assert_eq!(s_base1, 200);
        assert_eq!(b_base1, b"from b");

        // Cross-directory mismatch should fail (wrong subdir prefix for that index)
        let (s_cross0, _) = http_get(port, &format!("/{}/download/0/b/dup.txt", token));
        assert_eq!(s_cross0, 404);

        let (s_cross1, _) = http_get(port, &format!("/{}/download/1/a/dup.txt", token));
        assert_eq!(s_cross1, 404);

        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn serve_text_handles_escaped_quotes_and_newlines() {
        let (token, port, handle, state) = start_test_server();
        let (status, _) = http_post_text(port, &token, "hello \\\"quoted\\\"\\nline2\\\\tail");
        assert_eq!(status, 200);

        {
            let st = lock_state(&state);
            assert_eq!(
                st.shared_text.as_deref(),
                Some("hello \"quoted\"\nline2\\tail")
            );
        }

        let (st_code, st_body) = http_get(port, &format!("/{}/status", token));
        assert_eq!(st_code, 200);
        let st_json = String::from_utf8(st_body).unwrap();
        assert!(st_json.contains(&format!("\"token\":\"{}\"", token)));

        lock_state(&state).active = false;
        handle.join().ok();
    }

    fn read_proc_rss_kb() -> (u64, u64) {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let mut rss = 0u64;
        let mut hwm = 0u64;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                rss = rest
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("VmHWM:") {
                hwm = rest
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
            }
        }
        (rss, hwm)
    }

    fn read_proc_cpu_ticks() -> u64 {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
        if let Some(rparen) = stat.rfind(')') {
            let fields: Vec<&str> = stat[rparen + 2..].split_whitespace().collect();
            // After comm+state: index 11 is utime, index 12 is stime (fields[11], fields[12] relative to state at index 0)
            let utime: u64 = fields.get(11).and_then(|v| v.parse().ok()).unwrap_or(0);
            let stime: u64 = fields.get(12).and_then(|v| v.parse().ok()).unwrap_or(0);
            return utime + stime;
        }
        0
    }

    #[test]
    fn performance_cpu_and_memory_profile() {
        let work_dir = unique_temp_dir("perf_profile");
        std::fs::create_dir_all(&work_dir).unwrap();
        let _guard = TempDirGuard(work_dir.clone());

        // Create 32 MiB test file for streaming download & range tests
        let big_file = work_dir.join("big32m.bin");
        {
            let mut f = std::fs::File::create(&big_file).unwrap();
            let block = vec![0x5Au8; 1024 * 1024];
            for _ in 0..32 {
                f.write_all(&block).unwrap();
            }
            f.flush().unwrap();
        }

        // Create 4 x 4 MiB = 16 MiB directory for streaming ZIP test
        let zip_dir = work_dir.join("zipsrc");
        std::fs::create_dir_all(&zip_dir).unwrap();
        {
            let block = vec![0xA5u8; 1024 * 1024];
            for i in 0..4 {
                let mut f = std::fs::File::create(zip_dir.join(format!("part{i}.bin"))).unwrap();
                for _ in 0..4 {
                    f.write_all(&block).unwrap();
                }
            }
        }

        let upload_dir = work_dir.join("uploads");
        std::fs::create_dir_all(&upload_dir).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "perfprofiletok";
        let session = ServerSession::new(listener, port, token).unwrap();
        let state = session.state.clone();
        session.configure_send(
            vec![big_file.clone()],
            false,
            None,
            None,
            format!("http://127.0.0.1:{}/{}", port, token),
        );
        let handle = thread::spawn(move || {
            let _ = session.run();
        });
        thread::sleep(Duration::from_millis(50));

        // 1. Idle baseline (200ms)
        let (rss_base, _) = read_proc_rss_kb();
        let ticks_0 = read_proc_cpu_ticks();
        thread::sleep(Duration::from_millis(200));
        let ticks_idle = read_proc_cpu_ticks().saturating_sub(ticks_0);

        // 2. 32 MiB streaming download (client streams in 64 KiB buffer without 32 MiB allocation)
        let t_dl_start = Instant::now();
        let ticks_dl_0 = read_proc_cpu_ticks();
        let (rss_dl_before, _) = read_proc_rss_kb();
        let mut dl_stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        dl_stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let req = format!(
            "GET /{}/download/0/big32m.bin HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            token
        );
        dl_stream.write_all(req.as_bytes()).unwrap();
        let mut rbuf = [0u8; 64 * 1024];
        let mut total_rx = 0usize;
        let mut header_skipped = false;
        let mut hdr_acc = Vec::new();
        let mut rss_dl_peak = rss_dl_before;
        loop {
            let n = dl_stream.read(&mut rbuf).unwrap();
            if n == 0 {
                break;
            }
            if !header_skipped {
                hdr_acc.extend_from_slice(&rbuf[..n]);
                if let Some(pos) = hdr_acc.windows(4).position(|w| w == b"\r\n\r\n") {
                    total_rx += hdr_acc.len() - (pos + 4);
                    header_skipped = true;
                }
            } else {
                total_rx += n;
            }
            let (cur_rss, _) = read_proc_rss_kb();
            if cur_rss > rss_dl_peak {
                rss_dl_peak = cur_rss;
            }
        }
        let dl_elapsed = t_dl_start.elapsed();
        let ticks_dl = read_proc_cpu_ticks().saturating_sub(ticks_dl_0);
        assert_eq!(total_rx, 32 * 1024 * 1024);
        assert!(
            rss_dl_peak.saturating_sub(rss_dl_before) < 8192,
            "streaming download RSS grew too much: {} KiB -> {} KiB",
            rss_dl_before,
            rss_dl_peak
        );

        // 3. 16 MiB streaming ZIP download
        {
            let mut st = lock_state(&state);
            st.zip_roots = vec![zip_dir.clone()];
        }
        let t_zip_start = Instant::now();
        let ticks_zip_0 = read_proc_cpu_ticks();
        let (rss_zip_before, _) = read_proc_rss_kb();
        let mut zip_stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        zip_stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let zreq = format!(
            "GET /{}/download-all.zip HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            token
        );
        zip_stream.write_all(zreq.as_bytes()).unwrap();
        let mut zip_rx = 0usize;
        let mut rss_zip_peak = rss_zip_before;
        loop {
            let n = zip_stream.read(&mut rbuf).unwrap();
            if n == 0 {
                break;
            }
            zip_rx += n;
            let (cur_rss, _) = read_proc_rss_kb();
            if cur_rss > rss_zip_peak {
                rss_zip_peak = cur_rss;
            }
        }
        let zip_elapsed = t_zip_start.elapsed();
        let ticks_zip = read_proc_cpu_ticks().saturating_sub(ticks_zip_0);
        assert!(zip_rx > 16 * 1024 * 1024);
        assert!(
            rss_zip_peak.saturating_sub(rss_zip_before) < 8192,
            "streaming ZIP RSS grew too much: {} KiB -> {} KiB",
            rss_zip_before,
            rss_zip_peak
        );

        // 4. 32 MiB streaming upload
        {
            let mut st = lock_state(&state);
            st.mode = "receive".to_string();
            st.dest_dir = upload_dir.clone();
        }
        let t_up_start = Instant::now();
        let ticks_up_0 = read_proc_cpu_ticks();
        let (rss_up_before, _) = read_proc_rss_kb();
        let mut up_stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        up_stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let up_bytes_len = 32usize * 1024 * 1024;
        let up_hdr = format!(
            "POST /{}/upload?name=stream32m.bin HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            token, up_bytes_len
        );
        up_stream.write_all(up_hdr.as_bytes()).unwrap();
        let wbuf = vec![0x3Cu8; 64 * 1024];
        let mut rss_up_peak = rss_up_before;
        for _ in 0..(up_bytes_len / wbuf.len()) {
            up_stream.write_all(&wbuf).unwrap();
        }
        let (up_status, _, _) = read_response(&mut up_stream);
        let (cur_rss, hwm_end) = read_proc_rss_kb();
        if cur_rss > rss_up_peak {
            rss_up_peak = cur_rss;
        }
        let up_elapsed = t_up_start.elapsed();
        let ticks_up = read_proc_cpu_ticks().saturating_sub(ticks_up_0);
        assert_eq!(up_status, 200);
        assert_eq!(
            std::fs::metadata(upload_dir.join("stream32m.bin"))
                .unwrap()
                .len(),
            up_bytes_len as u64
        );
        assert!(
            rss_up_peak.saturating_sub(rss_up_before) < 8192,
            "streaming upload RSS grew too much: {} KiB -> {} KiB",
            rss_up_before,
            rss_up_peak
        );

        let dl_mb_s = 32.0 / dl_elapsed.as_secs_f64().max(1e-6);
        let zip_mb_s = 16.0 / zip_elapsed.as_secs_f64().max(1e-6);
        let up_mb_s = 32.0 / up_elapsed.as_secs_f64().max(1e-6);
        eprintln!(
            "[perf_profile] base_rss={}KB hwm={}KB idle_ticks={} | dl_32m={:.1}ms ({:.1} MB/s) rss_delta={}KB cpu_ticks={} | zip_16m={:.1}ms ({:.1} MB/s) rss_delta={}KB cpu_ticks={} | up_32m={:.1}ms ({:.1} MB/s) rss_delta={}KB cpu_ticks={}",
            rss_base,
            hwm_end,
            ticks_idle,
            dl_elapsed.as_secs_f64() * 1000.0,
            dl_mb_s,
            rss_dl_peak.saturating_sub(rss_dl_before),
            ticks_dl,
            zip_elapsed.as_secs_f64() * 1000.0,
            zip_mb_s,
            rss_zip_peak.saturating_sub(rss_zip_before),
            ticks_zip,
            up_elapsed.as_secs_f64() * 1000.0,
            up_mb_s,
            rss_up_peak.saturating_sub(rss_up_before),
            ticks_up,
        );

        lock_state(&state).active = false;
        handle.join().ok();
    }

    #[test]
    fn inactivity_timeout_resets_on_transfer_and_expires_when_idle() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "testtimeout";
        let session = ServerSession::new(listener, port, token).unwrap();
        let dest_dir = unique_temp_dir("inactivity_timeout");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let _guard = TempDirGuard(dest_dir.clone());
        let state = session.state.clone();
        {
            let mut st = lock_state(&state);
            st.mode = "receive".to_string();
            st.dest_dir = dest_dir.clone();
            st.timeout_secs = Some(1); // 1 second timeout
        }

        let handle = thread::spawn(move || {
            let _ = session.run();
        });

        // Wait for server to start
        thread::sleep(Duration::from_millis(50));

        // Verify server is active initially
        assert!(lock_state(&state).active);

        // Wait ~600ms, then do a small upload to reset the timer
        thread::sleep(Duration::from_millis(600));

        // Small upload should reset the inactivity timer
        let (status, _) = http_post_upload(port, &token, "reset.txt", b"reset");
        assert_eq!(status, 200);

        // Wait another 600ms (total ~1.2s elapsed since start, but only ~600ms since upload)
        thread::sleep(Duration::from_millis(600));

        // Server should still be active because upload reset the timer
        assert!(lock_state(&state).active);

        // Now wait for timeout to expire (1 second after last activity)
        thread::sleep(Duration::from_millis(1100));

        // Server should now be inactive
        assert!(!lock_state(&state).active);

        handle.join().ok();
    }

    #[test]
    fn upload_accepts_chunked_transfer_encoding() {
        let (token, port, handle, state) = start_test_server();
        let dest_dir = unique_temp_dir("chunked_upload");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let _guard = TempDirGuard(dest_dir.clone());
        {
            let mut st = lock_state(&state);
            st.mode = "receive".to_string();
            st.dest_dir = dest_dir.clone();
        }
        thread::sleep(Duration::from_millis(50));

        // Send chunked upload: 3 chunks
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let payload = b"hello chunked world";
        let req = format!(
            "POST /{}/upload?name=chunked.bin HTTP/1.1\r\n\
             Host: localhost\r\n\
             Transfer-Encoding: chunked\r\n\
             Connection: close\r\n\
             \r\n",
            token
        );
        stream.write_all(req.as_bytes()).unwrap();
        // Send chunks that match payload
        let chunks: &[&[u8]] = &[b"hello chunked world"];
        for chunk in chunks {
            let chunk_line = format!("{:x};source=test\r\n", chunk.len());
            stream.write_all(chunk_line.as_bytes()).unwrap();
            stream.write_all(chunk).unwrap();
            stream.write_all(b"\r\n").unwrap();
        }
        // Final chunk (0)
        stream
            .write_all(b"0\r\nX-Checksum: present\r\n\r\n")
            .unwrap();

        let (status, body, _) = read_response(&mut stream);
        assert_eq!(
            status,
            200,
            "Upload should succeed: {:?}",
            String::from_utf8_lossy(&body)
        );

        // Verify file was created with exact content
        let files: Vec<_> = std::fs::read_dir(&dest_dir).unwrap().collect();
        assert_eq!(files.len(), 1);
        let content = std::fs::read(files[0].as_ref().unwrap().path()).unwrap();
        assert_eq!(content, payload);

        // Verify session state
        {
            let st = lock_state(&state);
            assert_eq!(st.uploads_received, 1);
            assert_eq!(st.files, vec!["chunked.bin"]);
        }

        // No .part files should remain
        let part_files: Vec<_> = std::fs::read_dir(&dest_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".lanbeam.part"))
            .collect();
        assert_eq!(part_files.len(), 0);

        // Stop server
        let mut stream2 = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let stop_req = format!(
            "POST /{}/stop HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            token
        );
        stream2.write_all(stop_req.as_bytes()).unwrap();
        let _ = read_response(&mut stream2);

        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dest_dir);
    }

    #[test]
    fn upload_chunked_rejects_premature_eof_and_cleans_part_file() {
        let (token, port, handle, state) = start_test_server();
        let dest_dir = unique_temp_dir("chunked_eof");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let _guard = TempDirGuard(dest_dir.clone());
        {
            let mut st = lock_state(&state);
            st.mode = "receive".to_string();
            st.dest_dir = dest_dir.clone();
        }
        thread::sleep(Duration::from_millis(50));

        // Send chunked upload with truncated body (missing final 0 chunk)
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let req = format!(
            "POST /{}/upload?name=truncated.bin HTTP/1.1\r\n\
             Host: localhost\r\n\
             Transfer-Encoding: chunked\r\n\
             Connection: close\r\n\
             \r\n",
            token
        );
        stream.write_all(req.as_bytes()).unwrap();
        // Send one chunk
        let chunk = b"partial data";
        let chunk_line = format!("{:x}\r\n", chunk.len());
        stream.write_all(chunk_line.as_bytes()).unwrap();
        stream.write_all(chunk).unwrap();
        stream.write_all(b"\r\n").unwrap();
        // Don't send final 0 chunk - close write half
        stream.shutdown(std::net::Shutdown::Write).unwrap();

        let (status, _, _) = read_response(&mut stream);
        assert_eq!(status, 400, "Should reject truncated chunked upload");

        // No .part files should remain
        let part_files: Vec<_> = std::fs::read_dir(&dest_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".lanbeam.part"))
            .collect();
        assert_eq!(part_files.len(), 0, "No .part files should remain");

        // Stop server
        let mut stream2 = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let stop_req = format!(
            "POST /{}/stop HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            token
        );
        stream2.write_all(stop_req.as_bytes()).unwrap();
        let _ = read_response(&mut stream2);

        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dest_dir);
    }

    #[test]
    fn serve_text_accepts_chunked_transfer_encoding() {
        let (token, port, handle, state) = start_test_server();
        let dest_dir = unique_temp_dir("chunked_text");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let _guard = TempDirGuard(dest_dir.clone());
        {
            let mut st = lock_state(&state);
            st.mode = "text".to_string();
        }
        thread::sleep(Duration::from_millis(50));

        // Send chunked text message
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let text = "hello chunked";
        let req = format!(
            "POST /{}/text HTTP/1.1\r\n\
             Host: localhost\r\n\
             Transfer-Encoding: chunked\r\n\
             Connection: close\r\n\
             \r\n",
            token
        );
        stream.write_all(req.as_bytes()).unwrap();
        // Send body in chunks
        let body_str = format!("{{\"text\":\"{}\"}}", text);
        let body_chunks: &[&[u8]] = &[body_str.as_bytes()];
        for chunk in body_chunks {
            let chunk_line = format!("{:x}\r\n", chunk.len());
            stream.write_all(chunk_line.as_bytes()).unwrap();
            stream.write_all(chunk).unwrap();
            stream.write_all(b"\r\n").unwrap();
        }
        stream.write_all(b"0\r\n\r\n").unwrap();

        let (status, _body, _) = read_response(&mut stream);
        assert_eq!(status, 200);

        // Verify text was stored
        {
            let st = lock_state(&state);
            assert_eq!(st.shared_text.as_deref(), Some(text));
        }

        // Stop server
        let mut stream2 = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let stop_req = format!(
            "POST /{}/stop HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            token
        );
        stream2.write_all(stop_req.as_bytes()).unwrap();
        let _ = read_response(&mut stream2);

        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dest_dir);
    }

    #[test]
    fn chunked_text_enforces_limit_and_rejects_ambiguous_framing() {
        let (token, port, handle, state) = start_test_server();
        thread::sleep(Duration::from_millis(50));

        let mut oversized = TcpStream::connect(("127.0.0.1", port)).unwrap();
        oversized
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let req = format!(
            "POST /{}/text HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n10001\r\n",
            token
        );
        oversized.write_all(req.as_bytes()).unwrap();
        let (status, _, _) = read_response(&mut oversized);
        assert_eq!(status, 413);

        let mut ambiguous = TcpStream::connect(("127.0.0.1", port)).unwrap();
        ambiguous
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let req = format!(
            "POST /{}/text HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            token
        );
        ambiguous.write_all(req.as_bytes()).unwrap();
        let (status, _, _) = read_response(&mut ambiguous);
        assert_eq!(status, 400);

        let mut stop = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let req = format!(
            "POST /{}/stop HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            token
        );
        stop.write_all(req.as_bytes()).unwrap();
        let _ = read_response(&mut stop);
        handle.join().unwrap();
        assert_eq!(lock_state(&state).shared_text, None);
    }

    #[test]
    fn chunked_body_processes_large_chunks_in_bounded_slices() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let payload = vec![b'x'; 256 * 1024];
        let client_payload = payload.clone();
        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            write!(stream, "{:x}\r\n", client_payload.len()).unwrap();
            stream.write_all(&client_payload).unwrap();
            stream.write_all(b"\r\n0\r\n\r\n").unwrap();
        });

        let (mut stream, _) = listener.accept().unwrap();
        let state = Arc::new(Mutex::new(SessionState::default()));
        let mut received = 0usize;
        let mut largest_slice = 0usize;
        let total = read_chunked_body(&mut stream, &[], &state, 512 * 1024, |slice| {
            received += slice.len();
            largest_slice = largest_slice.max(slice.len());
            Ok(())
        })
        .unwrap();
        client.join().unwrap();

        assert_eq!(total, payload.len() as u64);
        assert_eq!(received, payload.len());
        assert!(largest_slice <= 64 * 1024, "slice size was {largest_slice}");
    }

    struct TempDirGuard(std::path::PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
