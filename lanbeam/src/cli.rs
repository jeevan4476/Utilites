use std::path::PathBuf;

/// Top-level CLI commands for lanbeam.
#[derive(Debug, Clone)]
pub enum Command {
    Send {
        paths: Vec<PathBuf>,
        port: Option<u16>,
        timeout: Option<u64>,
        once: bool,
        max_downloads: Option<u32>,
        gui: bool,
        json: bool,
    },
    Receive {
        dest: Option<PathBuf>,
        port: Option<u16>,
        timeout: Option<u64>,
        max_uploads: Option<u32>,
        gui: bool,
        json: bool,
    },
    Text {
        text: Option<String>,
        from_clipboard: bool,
        port: Option<u16>,
        timeout: Option<u64>,
        once: bool,
        gui: bool,
    },
    Qr {
        text_or_url: String,
        svg: bool,
    },
    SetupNautilus,
    Version,
    Help,
}

/// Minimal hand-rolled CLI parser (no external deps).
pub struct Parser {
    args: Vec<String>,
}

impl Parser {
    pub fn new(args: Vec<String>) -> Self {
        Self { args }
    }

    pub fn parse(mut self) -> Result<Command, String> {
        // Skip the program name.
        if !self.args.is_empty() {
            self.args.remove(0);
        }

        let subcommand = self.next_arg().ok_or_else(|| "expected a subcommand".to_string())?;

        match subcommand.as_str() {
            "send" => self.parse_send(),
            "receive" => self.parse_receive(),
            "text" => self.parse_text(),
            "qr" => self.parse_qr(),
            "setup-nautilus" => Ok(Command::SetupNautilus),
            "--version" | "-V" => Ok(Command::Version),
            "--help" | "-h" => Ok(Command::Help),
            other => Err(format!("unknown subcommand: {other}")),
        }
    }

    fn next_arg(&mut self) -> Option<String> {
        if self.args.is_empty() {
            return None;
        }
        Some(self.args.remove(0))
    }

    #[allow(dead_code)]
    fn peek(&self) -> Option<&str> {
        self.args.first().map(|s| s.as_str())
    }

    fn take_flag_value(&mut self, flag: &str, what: &str) -> Result<Option<String>, String> {
        match self.args.first() {
            Some(v) if !v.starts_with('-') => Ok(Some(self.args.remove(0))),
            _ => Err(format!("{flag} requires a {what} value")),
        }
    }

    fn parse_send(&mut self) -> Result<Command, String> {
        let mut paths = Vec::new();
        let mut port = None;
        let mut timeout = None;
        let mut once = false;
        let mut max_downloads = None;
        let mut gui = false;
        let mut json = false;

        while let Some(arg) = self.next_arg() {
            match arg.as_str() {
                "--port" => port = self.parse_port()?,
                "--timeout" => timeout = self.parse_timeout()?,
                "--once" => once = true,
                "--max-downloads" => max_downloads = self.parse_max_downloads()?,
                "--gui" => gui = true,
                "--json" => json = true,
                other if other.starts_with('-') => return Err(format!("unknown flag: {other}")),
                path => paths.push(PathBuf::from(path)),
            }
        }

        if paths.is_empty() {
            return Err("send requires at least one <path>".to_string());
        }

        Ok(Command::Send {
            paths,
            port,
            timeout,
            once,
            max_downloads,
            gui,
            json,
        })
    }

    fn parse_receive(&mut self) -> Result<Command, String> {
        let mut dest = None;
        let mut port = None;
        let mut timeout = None;
        let mut max_uploads = None;
        let mut gui = false;
        let mut json = false;

        while let Some(arg) = self.next_arg() {
            match arg.as_str() {
                "--dest" => dest = Some(PathBuf::from(self.take_required_value("--dest", "<DIR>")?)),
                "--port" => port = self.parse_port()?,
                "--timeout" => timeout = self.parse_timeout()?,
                "--max-uploads" => max_uploads = self.parse_max_uploads()?,
                "--gui" => gui = true,
                "--json" => json = true,
                other if other.starts_with('-') => return Err(format!("unknown flag: {other}")),
                other => return Err(format!("unexpected argument: {other}")),
            }
        }

        Ok(Command::Receive {
            dest,
            port,
            timeout,
            max_uploads,
            gui,
            json,
        })
    }

    fn parse_text(&mut self) -> Result<Command, String> {
        let mut text = None;
        let mut from_clipboard = false;
        let mut port = None;
        let mut timeout = None;
        let mut once = false;
        let mut gui = false;

        while let Some(arg) = self.next_arg() {
            match arg.as_str() {
                "--from-clipboard" => from_clipboard = true,
                "--port" => port = self.parse_port()?,
                "--timeout" => timeout = self.parse_timeout()?,
                "--once" => once = true,
                "--gui" => gui = true,
                other if other.starts_with('-') => return Err(format!("unknown flag: {other}")),
                value => text = Some(value.to_string()),
            }
        }

        Ok(Command::Text {
            text,
            from_clipboard,
            port,
            timeout,
            once,
            gui,
        })
    }

    fn parse_qr(&mut self) -> Result<Command, String> {
        let mut text_or_url = String::new();
        let mut svg = false;

        while let Some(arg) = self.next_arg() {
            match arg.as_str() {
                "--svg" => svg = true,
                other if other.starts_with('-') => return Err(format!("unknown flag: {other}")),
                value => text_or_url = value.to_string(),
            }
        }

        if text_or_url.is_empty() {
            return Err("qr requires <text_or_url>".to_string());
        }

        Ok(Command::Qr {
            text_or_url,
            svg,
        })
    }

    fn parse_port(&mut self) -> Result<Option<u16>, String> {
        match self.take_flag_value("--port", "port")? {
            Some(v) => Ok(Some(v.parse::<u16>().map_err(|_| "invalid port".to_string())?)),
            None => Ok(None),
        }
    }

    fn parse_timeout(&mut self) -> Result<Option<u64>, String> {
        match self.take_flag_value("--timeout", "timeout")? {
            Some(v) => Ok(Some(v.parse::<u64>().map_err(|_| "invalid timeout".to_string())?)),
            None => Ok(None),
        }
    }

    fn parse_max_downloads(&mut self) -> Result<Option<u32>, String> {
        match self.take_flag_value("--max-downloads", "max-downloads")? {
            Some(v) => Ok(Some(v.parse::<u32>().map_err(|_| "invalid max-downloads".to_string())?)),
            None => Ok(None),
        }
    }

    fn parse_max_uploads(&mut self) -> Result<Option<u32>, String> {
        match self.take_flag_value("--max-uploads", "max-uploads")? {
            Some(v) => Ok(Some(v.parse::<u32>().map_err(|_| "invalid max-uploads".to_string())?)),
            None => Ok(None),
        }
    }

    fn take_required_value(&mut self, flag: &str, what: &str) -> Result<String, String> {
        self.take_flag_value(flag, what)?.ok_or_else(|| format!("{flag} requires a {what} value"))
    }
}

pub const USAGE: &str = r#"lanbeam 0.1.0 - local Wi-Fi file transfer

USAGE:
    lanbeam send <path...> [--port PORT] [--timeout SEC] [--once] [--max-downloads N] [--gui] [--json]
    lanbeam receive [--dest DIR] [--port PORT] [--timeout SEC] [--max-uploads N] [--gui] [--json]
    lanbeam text [TEXT] [--from-clipboard] [--port PORT] [--timeout SEC] [--once] [--gui]
    lanbeam qr <text_or_url> [--svg]
    lanbeam setup-nautilus

FLAGS:
    --once               Stop after the first transfer completes
    --gui                Show the GTK4 desktop popup
    --json               Emit machine-readable JSON output

OPTIONS:
    --port PORT          TCP port to listen on (1025..=65535)
    --timeout SEC        Seconds of inactivity before auto-shutdown
    --max-downloads N    Stop after N downloads complete
    --max-uploads N      Stop after N uploads complete
    --dest DIR           Directory to save received uploads into

SUBCOMMANDS:
    send             Share local files/folders over the LAN
    receive          Listen for uploads from a phone
    text             Beam text/links in both directions
    qr               Print a QR code for text or a URL
    setup-nautilus   Install binary and GNOME Files right-click script
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        Parser::new(args.iter().map(|s| (*s).to_string()).collect()).parse()
    }

    #[test]
    fn send_basic() {
        let cmd = parse(&["lanbeam", "send", "/path/to/file.txt"]).unwrap();
        assert!(matches!(cmd, Command::Send { ref paths, .. } if paths.len() == 1));
    }

    #[test]
    fn send_multiple_paths_and_flags() {
        let cmd = parse(&[
            "lanbeam", "send", "/a.txt", "/b.txt", "--port", "8080", "--timeout", "30",
            "--once", "--max-downloads", "5", "--gui", "--json",
        ]).unwrap();
        match cmd {
            Command::Send { paths, port, timeout, once, max_downloads, gui, json } => {
                assert_eq!(paths.len(), 2);
                assert_eq!(port, Some(8080));
                assert_eq!(timeout, Some(30));
                assert!(once);
                assert_eq!(max_downloads, Some(5));
                assert!(gui);
                assert!(json);
            }
            _ => panic!("expected Send"),
        }
    }

    #[test]
    fn send_requires_paths() {
        let err = parse(&["lanbeam", "send"]).unwrap_err();
        assert!(err.contains("send requires at least one"));
    }

    #[test]
    fn send_unknown_flag() {
        let err = parse(&["lanbeam", "send", "/a.txt", "--bogus"]).unwrap_err();
        assert!(err.contains("unknown flag"));
    }

    #[test]
    fn receive_with_dest() {
        let cmd = parse(&["lanbeam", "receive", "--dest", "/tmp/recv"]).unwrap();
        match cmd {
            Command::Receive { dest, port, timeout, max_uploads, gui, json } => {
                assert_eq!(dest, Some(std::path::PathBuf::from("/tmp/recv")));
                assert_eq!(port, None);
                assert_eq!(timeout, None);
                assert_eq!(max_uploads, None);
                assert!(!gui);
                assert!(!json);
            }
            _ => panic!("expected Receive"),
        }
    }

    #[test]
    fn receive_all_flags() {
        let cmd = parse(&[
            "lanbeam", "receive", "--dest", "/d", "--port", "9000", "--timeout", "60",
            "--max-uploads", "10", "--gui", "--json",
        ]).unwrap();
        match cmd {
            Command::Receive { dest, port, timeout, max_uploads, gui, json } => {
                assert_eq!(dest, Some(std::path::PathBuf::from("/d")));
                assert_eq!(port, Some(9000));
                assert_eq!(timeout, Some(60));
                assert_eq!(max_uploads, Some(10));
                assert!(gui);
                assert!(json);
            }
            _ => panic!("expected Receive"),
        }
    }

    #[test]
    fn text_with_value() {
        let cmd = parse(&["lanbeam", "text", "hello world"]).unwrap();
        match cmd {
            Command::Text { text, .. } => assert_eq!(text, Some("hello world".to_string())),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn text_from_clipboard() {
        let cmd = parse(&["lanbeam", "text", "--from-clipboard", "--once", "--gui"]).unwrap();
        match cmd {
            Command::Text { text, from_clipboard, once, gui, .. } => {
                assert_eq!(text, None);
                assert!(from_clipboard);
                assert!(once);
                assert!(gui);
            }
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn qr_with_text() {
        let cmd = parse(&["lanbeam", "qr", "https://example.com"]).unwrap();
        match cmd {
            Command::Qr { text_or_url, svg } => {
                assert_eq!(text_or_url, "https://example.com");
                assert!(!svg);
            }
            _ => panic!("expected Qr"),
        }
    }

    #[test]
    fn qr_with_svg_flag() {
        let cmd = parse(&["lanbeam", "qr", "hi", "--svg"]).unwrap();
        match cmd {
            Command::Qr { text_or_url, svg } => {
                assert_eq!(text_or_url, "hi");
                assert!(svg);
            }
            _ => panic!("expected Qr"),
        }
    }

    #[test]
    fn qr_requires_text() {
        let err = parse(&["lanbeam", "qr"]).unwrap_err();
        assert!(err.contains("qr requires"));
    }

    #[test]
    fn setup_nautilus() {
        let cmd = parse(&["lanbeam", "setup-nautilus"]).unwrap();
        assert!(matches!(cmd, Command::SetupNautilus));
    }

    #[test]
    fn unknown_subcommand() {
        let err = parse(&["lanbeam", "frobnicate"]).unwrap_err();
        assert!(err.contains("unknown subcommand"));
    }

    #[test]
    fn no_subcommand() {
        let err = parse(&["lanbeam"]).unwrap_err();
        assert!(err.contains("expected a subcommand"));
    }

    #[test]
    fn invalid_port() {
        let err = parse(&["lanbeam", "send", "/a.txt", "--port", "notaport"]).unwrap_err();
        assert!(err.contains("invalid port"));
    }

    #[test]
    fn port_requires_value() {
        let err = parse(&["lanbeam", "send", "/a.txt", "--port"]).unwrap_err();
        assert!(err.contains("requires a port value"));
    }

    #[test]
    fn version_and_help_parse_as_success_commands() {
        assert!(matches!(parse(&["lanbeam", "--version"]).unwrap(), Command::Version));
        assert!(matches!(parse(&["lanbeam", "-V"]).unwrap(), Command::Version));
        assert!(matches!(parse(&["lanbeam", "--help"]).unwrap(), Command::Help));
        assert!(matches!(parse(&["lanbeam", "-h"]).unwrap(), Command::Help));
    }
}
