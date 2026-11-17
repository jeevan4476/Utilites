//! Multi-threaded streaming HTTP server (Step 6).
//!
//! Implements `ServerSession` over `std::net::TcpListener` serving the lanbeam
//! transfer app on `GET /<token>` with file download, upload, text, and QR
//! endpoints. This is the skeleton; endpoint bodies are filled in below.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

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
}

impl ServerSession {
    /// Create a new session bound to `listener` on `port` serving `token`.
    pub fn new(listener: TcpListener, port: u16, token: &str) -> io::Result<Self> {
        Ok(Self {
            listener,
            token: token.to_string(),
            state: Arc::new(Mutex::new(SessionState {
                mode: "send".to_string(),
                url: String::new(),
                files: Vec::new(),
                downloads_completed: 0,
                uploads_received: 0,
                shared_text: None,
                remaining_seconds: 0,
                active: true,
            })),
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

    /// Run the listener loop, handling each connection on its own thread until
    /// `stop()` is called or the session is deactivated.
    pub fn run(self) -> io::Result<()> {
        self.listener.set_nonblocking(true)?;
        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    let state = self.state.clone();
                    let token = self.token.clone();
                    thread::spawn(move || {
                        if let Err(e) = handle_connection(stream, &token, &state) {
                            eprintln!("lanbeam: connection error: {e}");
                        }
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    let active = self.state.lock().unwrap().active;
                    if !active {
                        break;
                    }
                    thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Cleanly terminate the listener loop.
    pub fn stop(&self) {
        self.state.lock().unwrap().active = false;
    }
}

/// Handle a single HTTP connection.
fn handle_connection(
    mut stream: TcpStream,
    token: &str,
    state: &Arc<Mutex<SessionState>>,
) -> io::Result<()> {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf)?;
    if n == 0 {
        return Ok(());
    }
    let req = String::from_utf8_lossy(&buf[..n]);

    // Parse request line.
    let mut lines = req.lines();
    let request_line = lines.next().unwrap_or("");
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return respond(&mut stream, 400, "text/plain", b"Bad Request");
    }
    let method = parts[0];
    let path = parts[1];

    // Reject any path outside /<token>.
    let expected_prefix = format!("/{}", token);
    if path != "/" && !path.starts_with(&expected_prefix) {
        return respond(&mut stream, 403, "text/plain", b"Forbidden");
    }

    match (method, path) {
        ("GET", p) if p == &expected_prefix || p == format!("{expected_prefix}/") => {
            serve_app(&mut stream, state)
        }
        ("GET", p) if p == format!("{expected_prefix}/qr.svg") => serve_qr(&mut stream, token),
        ("GET", p) if p == format!("{expected_prefix}/status") => serve_status(&mut stream, state),
        ("GET", p) if p.starts_with(&format!("{expected_prefix}/download/")) => {
            serve_download(&mut stream, p)
        }
        ("GET", p) if p == format!("{expected_prefix}/download-all.zip") => {
            serve_download_all(&mut stream)
        }
        ("POST", p) if p == format!("{expected_prefix}/upload") => serve_upload(&mut stream, p),
        ("POST", p) if p == format!("{expected_prefix}/text") => {
            let body = req.split("\r\n\r\n").nth(1).unwrap_or("");
            serve_text(&mut stream, state, body)
        }
        ("POST", p) if p == format!("{expected_prefix}/stop") => {
            state.lock().unwrap().active = false;
            respond(&mut stream, 200, "text/plain", b"Stopping")
        }
        _ => respond(&mut stream, 404, "text/plain", b"Not Found"),
    }
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> io::Result<()> {
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
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

fn serve_qr(stream: &mut TcpStream, token: &str) -> io::Result<()> {
    let qr = QrCode::encode(token).unwrap();
    let svg = qr.to_svg(4);
    respond(stream, 200, "image/svg+xml", svg.as_bytes())
}

fn serve_status(stream: &mut TcpStream, state: &Arc<Mutex<SessionState>>) -> io::Result<()> {
    let st = state.lock().unwrap();
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

/// Minimal JSON string escaping (enough for our controlled values).
fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn serve_download(stream: &mut TcpStream, _path: &str) -> io::Result<()> {
    respond(stream, 404, "text/plain", b"Not Implemented")
}

fn serve_download_all(stream: &mut TcpStream) -> io::Result<()> {
    respond(stream, 404, "text/plain", b"Not Implemented")
}

fn serve_upload(stream: &mut TcpStream, _path: &str) -> io::Result<()> {
    respond(stream, 404, "text/plain", b"Not Implemented")
}

fn serve_text(
    stream: &mut TcpStream,
    state: &Arc<Mutex<SessionState>>,
    body: &str,
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
        let mut st = state.lock().unwrap();
        st.shared_text = Some(text.clone());
        drop(st);

        // Pipe to Wayland clipboard and notify the desktop.
        let _ = std::process::Command::new("wl-copy").arg(&text).spawn();
        let _ = std::process::Command::new("notify-send")
            .args(["lanbeam", &text])
            .spawn();
    }

    respond(stream, 200, "application/json", b"{\"ok\":true}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_state_defaults() {
        let st = SessionState::default();
        assert!(st.active);
        assert_eq!(st.downloads_completed, 0);
    }
}
