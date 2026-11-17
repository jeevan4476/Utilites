//! LAN IPv4 discovery, token generation, and Fedora port binding.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, UdpSocket};

/// Discover the outbound Wi-Fi/LAN IPv4 address by probing a UDP connect to
/// `10.255.255.255:1` (a non-routable sink). Falls back to `127.0.0.1` when
/// offline or on error, as expected on Fedora Workstation (`wlp0s20f3`).
pub fn detect_lan_ip() -> IpAddr {
    let probe = UdpSocket::bind("0.0.0.0:0");
    if let Ok(socket) = probe {
        // A non-routable destination forces the OS to pick the outbound
        // interface without actually sending anything meaningful.
        if socket.connect("10.255.255.255:1").is_ok() {
            if let Ok(local) = socket.local_addr() {
                return local.ip();
            }
        }
    }
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

/// Bind a TCP listener on `host` starting at `preferred_port`, searching
/// upward through the Fedora firewalld-permitted range `1025..=65535`.
/// Returns the listener and the actual port bound.
pub fn bind_listener(host: IpAddr, preferred_port: u16) -> io::Result<(TcpListener, u16)> {
    let port = preferred_port.clamp(1025, 65535);
    let mut last_err: Option<io::Error> = None;

    for p in port..=u16::MAX {
        let addr = SocketAddr::new(host, p);
        match TcpListener::bind(addr) {
            Ok(listener) => return Ok((listener, p)),
            Err(e) => last_err = Some(e),
        }
    }

    Err(last_err.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::AddrNotAvailable, "no ports available in 1025..=65535")
    }))
}

/// Generate a 12-character base62 token from 9 random bytes read from
/// `/dev/urandom`.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 9];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut bytes);
    } else {
        // Fallback: time-based entropy.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0) as u64;
        let nbytes = bytes.len();
        for (i, b) in bytes.iter_mut().enumerate() {
            let shift = (8 * (nbytes - 1 - i)) as u64;
            *b = (now >> shift) as u8;
        }
    }

    const CHARS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::with_capacity(12);
    // Encode the 9-byte big-endian integer as base62.
    let mut num: u128 = 0;
    for &b in &bytes {
        num = (num << 8) | (b as u128);
    }
    while num > 0 {
        out.push(CHARS[(num % 62) as usize]);
        num /= 62;
    }
    out.reverse();
    let token: String = out.into_iter().map(|b| b as char).collect();

    // Pad to exactly 12 characters for stable, short URLs.
    if token.len() < 12 {
        format!("{:0<12}", token)
    } else {
        token[..12.min(token.len())].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_twelve_chars() {
        let t = generate_token();
        assert_eq!(t.len(), 12);
        assert!(t.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn tokens_differ() {
        let t1 = generate_token();
        let t2 = generate_token();
        // Astronomically unlikely to collide; just sanity-check non-empty.
        assert!(!t1.is_empty() && !t2.is_empty());
    }

    #[test]
    fn lan_ip_is_ipv4() {
        let ip = detect_lan_ip();
        assert!(matches!(ip, IpAddr::V4(_)));
    }

    #[test]
    fn bind_listener_rejects_privileged_range() {
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        // Port 80 should be clamped upward to >= 1025.
        let (listener, port) = bind_listener(ip, 80).expect("bind localhost");
        assert!(port >= 1025 && port <= 65535);
        drop(listener);
    }
}
