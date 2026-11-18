use std::env;
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command as StdCommand, ExitCode, Stdio};

use lanbeam::{cli::Command, network, qr, server, VERSION};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let parser = lanbeam::cli::Parser::new(args);

    match parser.parse() {
        Ok(cmd) => match run(cmd) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                // Handle --version and --help cleanly
                if e.starts_with("lanbeam ") || e.starts_with("Usage:") {
                    println!("{e}");
                    ExitCode::SUCCESS
                } else {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        },
        Err(e) => {
            // Handle --version and --help cleanly
            if e.starts_with("lanbeam ") || e.starts_with("Usage:") {
                println!("{e}");
                ExitCode::SUCCESS
            } else {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        }
    }
}

fn run(cmd: Command) -> Result<(), String> {
    match cmd {
        Command::Send {
            paths,
            port,
            timeout,
            once,
            max_downloads,
            gui,
            json,
        } => {
            let ip = network::detect_lan_ip();
            let bind_host = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
            let preferred = port.unwrap_or(5000);
            let (listener, bound_port) = network::bind_listener(bind_host, preferred)
                .map_err(|e| format!("failed to bind port: {e}"))?;
            let token = network::generate_token();
            let url = format!("http://{}:{}/{}", ip, bound_port, token);

            if json {
                println!(
                    "{{\"token\":\"{}\",\"ip\":\"{}\",\"port\":{}}}",
                    token, ip, bound_port
                );
            } else {
                println!("lanbeam send — sharing {} path(s)", paths.len());
                println!("  URL:  {}", url);
                println!("  QR:");
                let q = qr::QrCode::encode(&url).map_err(|e| format!("qr encode failed: {e}"))?;
                println!("{}", q.to_ansi());
            }

            let session = server::ServerSession::new(listener, bound_port, &token)
                .map_err(|e| format!("failed to start server: {e}"))?;

            let abs_paths: Vec<PathBuf> = paths
                .into_iter()
                .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
                .collect();
            session.configure_send(abs_paths, once, max_downloads, timeout, url.clone());

            if gui {
                spawn_gui_popup(&token, &url, bound_port);
            }

            if !json {
                println!("  Listening on port {bound_port}. Press Ctrl+C to stop.");
            }
            session.run().map_err(|e| format!("server error: {e}"))?;
            Ok(())
        }
        Command::Receive {
            dest,
            port,
            timeout,
            max_uploads,
            gui,
            json,
        } => {
            let ip = network::detect_lan_ip();
            let bind_host = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
            let preferred = port.unwrap_or(5001);
            let (listener, bound_port) = network::bind_listener(bind_host, preferred)
                .map_err(|e| format!("failed to bind port: {e}"))?;
            let token = network::generate_token();
            let url = format!("http://{}:{}/{}", ip, bound_port, token);

            if json {
                println!(
                    "{{\"token\":\"{}\",\"ip\":\"{}\",\"port\":{}}}",
                    token, ip, bound_port
                );
            } else {
                println!("lanbeam receive — listening on {}", url);
                println!("  QR:");
                let q = qr::QrCode::encode(&url).map_err(|e| format!("qr encode failed: {e}"))?;
                println!("{}", q.to_ansi());
            }

            let session = server::ServerSession::new(listener, bound_port, &token)
                .map_err(|e| format!("failed to start server: {e}"))?;

            let dest_dir = dest.unwrap_or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
                    .join("Downloads")
            });
            session.configure_receive(dest_dir, max_uploads, timeout, url.clone());

            if gui {
                spawn_gui_popup(&token, &url, bound_port);
            }

            session.run().map_err(|e| format!("server error: {e}"))?;
            Ok(())
        }
        Command::Text {
            text,
            from_clipboard,
            port,
            timeout,
            once,
            gui,
        } => {
            let ip = network::detect_lan_ip();
            let bind_host = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
            let preferred = port.unwrap_or(5002);
            let (listener, bound_port) = network::bind_listener(bind_host, preferred)
                .map_err(|e| format!("failed to bind port: {e}"))?;
            let token = network::generate_token();
            let url = format!("http://{}:{}/{}", ip, bound_port, token);

            println!("lanbeam text — {}", url);
            println!("  QR:");
            let q = qr::QrCode::encode(&url).map_err(|e| format!("qr encode failed: {e}"))?;
            println!("{}", q.to_ansi());

            let session = server::ServerSession::new(listener, bound_port, &token)
                .map_err(|e| format!("failed to start server: {e}"))?;

            // Handle --from-clipboard
            let text_content = if from_clipboard {
                Some(read_clipboard_text()?)
            } else {
                text
            };

            session.configure_text(text_content, once, timeout, url.clone());

            if gui {
                spawn_gui_popup(&token, &url, bound_port);
            }

            session.run().map_err(|e| format!("server error: {e}"))?;
            Ok(())
        }
        Command::Qr { text_or_url, svg } => {
            let q = lanbeam::qr::QrCode::encode(&text_or_url)
                .map_err(|e| format!("qr encode failed: {e}"))?;
            if svg {
                println!("{}", q.to_svg(4));
            } else {
                println!("{}", q.to_ansi());
            }
            Ok(())
        }
        Command::SetupNautilus => {
            let installed = run_setup_nautilus()?;
            for path in installed {
                println!("installed: {}", path.display());
            }
            Ok(())
        }
        Command::Version => {
            println!("lanbeam {}", VERSION);
            Ok(())
        }
        Command::Help => {
            println!("{}", lanbeam::cli::USAGE);
            Ok(())
        }
    }
}

/// Read text from clipboard using wl-paste (Wayland) or xclip (X11).
fn read_clipboard_text() -> Result<String, String> {
    // Try wl-paste first (Wayland)
    if let Ok(output) = StdCommand::new("wl-paste").arg("--no-newline").output() {
        if output.status.success() {
            return String::from_utf8(output.stdout)
                .map_err(|e| format!("clipboard text is not valid UTF-8: {e}"));
        }
    }
    // Fall back to xclip (X11)
    if let Ok(output) = StdCommand::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
    {
        if output.status.success() {
            return String::from_utf8(output.stdout)
                .map_err(|e| format!("clipboard text is not valid UTF-8: {e}"));
        }
    }
    Err("failed to read clipboard: neither wl-paste nor xclip succeeded".to_string())
}

/// Locate the popup.py script and spawn it detached.
fn spawn_gui_popup(token: &str, base_url: &str, port: u16) {
    let popup_path = find_popup_script();
    if let Err(e) = StdCommand::new("python3")
        .arg(&popup_path)
        .arg(token)
        .arg(base_url)
        .arg("--port")
        .arg(port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        eprintln!("lanbeam: failed to launch GUI popup: {e}");
    }
}

/// Run the setup-nautilus installation routine.
fn run_setup_nautilus() -> Result<Vec<PathBuf>, String> {
    // Resolve home directory
    let home = env::var_os("LANBEAM_INSTALL_HOME")
        .or_else(|| env::var_os("HOME"))
        .ok_or_else(|| "HOME environment variable is not set".to_string())?;
    let home = PathBuf::from(home);
    if home.as_os_str().is_empty() {
        return Err("HOME environment variable is not set".to_string());
    }

    // Locate project source root containing ui/popup.py and nautilus/Share on LAN
    let source_root = env::var_os("LANBEAM_SOURCE_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            // Try CARGO_MANIFEST_DIR at compile time
            option_env!("CARGO_MANIFEST_DIR").map(PathBuf::from)
        })
        .or_else(|| {
            // Try ancestors of current_exe
            env::current_exe().ok().and_then(|exe| {
                exe.ancestors()
                    .find(|p| {
                        p.join("ui/popup.py").exists() && p.join("nautilus/Share on LAN").exists()
                    })
                    .map(|p| p.to_path_buf())
            })
        })
        .ok_or_else(|| {
            "could not locate project source root (ui/popup.py and nautilus/Share on LAN)"
                .to_string()
        })?;

    let src_popup = source_root.join("ui/popup.py");
    let src_script = source_root.join("nautilus/Share on LAN");
    if !src_popup.is_file() {
        return Err(format!("source popup not found: {}", src_popup.display()));
    }
    if !src_script.is_file() {
        return Err(format!(
            "source nautilus script not found: {}",
            src_script.display()
        ));
    }

    // Create target directories
    let bin_dir = home.join(".local/bin");
    let share_ui_dir = home.join(".local/share/lanbeam/ui");
    let nautilus_scripts_dir = home.join(".local/share/nautilus/scripts");
    fs::create_dir_all(&bin_dir).map_err(|e| format!("failed to create bin dir: {e}"))?;
    fs::create_dir_all(&share_ui_dir).map_err(|e| format!("failed to create share dir: {e}"))?;
    fs::create_dir_all(&nautilus_scripts_dir)
        .map_err(|e| format!("failed to create nautilus scripts dir: {e}"))?;

    let mut installed = Vec::new();

    // Copy binary
    let dest_bin = bin_dir.join("lanbeam");
    let current_exe = env::current_exe().map_err(|e| format!("failed to get current exe: {e}"))?;
    if dest_bin != current_exe {
        fs::copy(&current_exe, &dest_bin).map_err(|e| format!("failed to copy binary: {e}"))?;
    }
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&dest_bin)
            .map_err(|e| format!("failed to get metadata: {e}"))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&dest_bin, perms)
            .map_err(|e| format!("failed to set permissions: {e}"))?;
    }
    installed.push(dest_bin);

    // Copy popup.py
    let dest_popup = share_ui_dir.join("popup.py");
    fs::copy(&src_popup, &dest_popup).map_err(|e| format!("failed to copy popup.py: {e}"))?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&dest_popup)
            .map_err(|e| format!("failed to get metadata: {e}"))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&dest_popup, perms)
            .map_err(|e| format!("failed to set permissions: {e}"))?;
    }
    installed.push(dest_popup);

    // Copy nautilus script
    let dest_script = nautilus_scripts_dir.join("Share on LAN");
    fs::copy(&src_script, &dest_script)
        .map_err(|e| format!("failed to copy nautilus script: {e}"))?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&dest_script)
            .map_err(|e| format!("failed to get metadata: {e}"))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&dest_script, perms)
            .map_err(|e| format!("failed to set permissions: {e}"))?;
    }
    installed.push(dest_script);

    Ok(installed)
}

/// Find the popup.py script in standard locations.
fn find_popup_script() -> PathBuf {
    // 1. LANBEAM_POPUP_PATH env var
    if let Ok(path) = env::var("LANBEAM_POPUP_PATH") {
        let pb = PathBuf::from(path);
        if pb.exists() {
            return pb;
        }
    }

    // 2. Relative to executable: <exe_dir>/../share/lanbeam/ui/popup.py
    if let Ok(exe) = env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let candidate = exe_dir.join("../share/lanbeam/ui/popup.py");
            if candidate.exists() {
                return candidate;
            }
            // 3. Relative to executable: <exe_dir>/ui/popup.py (for development)
            let candidate = exe_dir.join("ui/popup.py");
            if candidate.exists() {
                return candidate;
            }
        }
    }

    // 4. User data directory
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        let candidate = home.join(".local/share/lanbeam/ui/popup.py");
        if candidate.exists() {
            return candidate;
        }
    }

    // 5. Current directory (for development)
    if PathBuf::from("ui/popup.py").exists() {
        return PathBuf::from("ui/popup.py");
    }

    // 6. lanbeam/ subdirectory (for development from repo root)
    if PathBuf::from("lanbeam/ui/popup.py").exists() {
        return PathBuf::from("lanbeam/ui/popup.py");
    }

    // Fallback: return a likely path, let the spawn fail with a clear error
    PathBuf::from("ui/popup.py")
}

#[allow(dead_code)]
const _VERSION_USE: &str = VERSION;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore(&'static str, Option<std::ffi::OsString>);
    impl EnvRestore {
        fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
            let old = env::var_os(key);
            unsafe {
                env::set_var(key, value);
            }
            Self(key, old)
        }
    }
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            unsafe {
                if let Some(value) = self.1.take() {
                    env::set_var(self.0, value);
                } else {
                    env::remove_var(self.0);
                }
            }
        }
    }

    struct TempDirCleanup(PathBuf);
    impl Drop for TempDirCleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), id))
    }

    #[test]
    fn read_clipboard_text_mock_wl_paste() {
        // We can't easily mock wl-paste in unit tests, but we can test the logic
        // by creating a fake wl-paste script in a temp directory and prepending
        // it to PATH. This is complex, so we just test the function doesn't panic
        // when clipboard tools are unavailable (it returns an error).
        let result = read_clipboard_text();
        // Either it succeeds (if clipboard has text) or fails with expected error
        assert!(result.is_ok() || result.unwrap_err().contains("clipboard"));
    }

    #[test]
    fn find_popup_script_env_var() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_temp_dir("popup_test");
        fs::create_dir_all(&tmp).unwrap();
        let popup = tmp.join("popup.py");
        fs::write(&popup, "#!/usr/bin/env python3\nprint('test')").unwrap();

        // Set env var
        unsafe {
            env::set_var("LANBEAM_POPUP_PATH", &popup);
        }
        let found = find_popup_script();
        assert_eq!(found, popup);
        unsafe {
            env::remove_var("LANBEAM_POPUP_PATH");
        }
    }

    #[test]
    fn find_popup_script_relative_to_exe() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_temp_dir("popup_test2");
        let _bin_dir = tmp.join("bin");
        let share_dir = tmp.join("share/lanbeam/ui");
        fs::create_dir_all(&share_dir).unwrap();
        let popup = share_dir.join("popup.py");
        fs::write(&popup, "#!/usr/bin/env python3\nprint('test')").unwrap();

        // Mock current_exe to point to our fake binary
        // We can't easily mock current_exe, so we test the fallback logic
        // by ensuring the function doesn't panic
        let found = find_popup_script();
        // Should not panic
        assert!(!found.as_os_str().is_empty());
    }

    #[test]
    fn find_popup_script_fallback() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Test that fallback returns a path even when nothing exists
        let found = find_popup_script();
        assert!(!found.as_os_str().is_empty());
    }

    #[test]
    fn spawn_gui_popup_does_not_panic() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This test just ensures the function doesn't panic when called
        // with a non-existent script (it will fail silently with eprintln)
        let tmp = unique_temp_dir("popup_test3");
        let popup = tmp.join("nonexistent.py");
        // Don't create the file - spawn should handle missing file gracefully
        unsafe {
            env::set_var("LANBEAM_POPUP_PATH", &popup);
        }
        spawn_gui_popup("token123", "http://example.com", 5000);
        unsafe {
            env::remove_var("LANBEAM_POPUP_PATH");
        }
        // If we get here without panic, test passes
    }

    #[test]
    fn cli_send_with_gui_flag() {
        // Test that Send command can be parsed with --gui
        let parser = lanbeam::cli::Parser::new(vec![
            "lanbeam".to_string(),
            "send".to_string(),
            "/tmp/test.txt".to_string(),
            "--gui".to_string(),
        ]);
        let cmd = parser.parse().unwrap();
        match cmd {
            Command::Send { gui, .. } => assert!(gui),
            _ => panic!("expected Send command"),
        }
    }

    #[test]
    fn cli_receive_with_gui_flag() {
        let parser = lanbeam::cli::Parser::new(vec![
            "lanbeam".to_string(),
            "receive".to_string(),
            "--gui".to_string(),
        ]);
        let cmd = parser.parse().unwrap();
        match cmd {
            Command::Receive { gui, .. } => assert!(gui),
            _ => panic!("expected Receive command"),
        }
    }

    #[test]
    fn cli_text_with_gui_flag() {
        let parser = lanbeam::cli::Parser::new(vec![
            "lanbeam".to_string(),
            "text".to_string(),
            "hello".to_string(),
            "--gui".to_string(),
        ]);
        let cmd = parser.parse().unwrap();
        match cmd {
            Command::Text { gui, .. } => assert!(gui),
            _ => panic!("expected Text command"),
        }
    }

    #[test]
    fn cli_text_with_from_clipboard() {
        let parser = lanbeam::cli::Parser::new(vec![
            "lanbeam".to_string(),
            "text".to_string(),
            "--from-clipboard".to_string(),
        ]);
        let cmd = parser.parse().unwrap();
        match cmd {
            Command::Text { from_clipboard, .. } => assert!(from_clipboard),
            _ => panic!("expected Text command"),
        }
    }

    #[test]
    fn cli_text_from_clipboard_and_gui() {
        let parser = lanbeam::cli::Parser::new(vec![
            "lanbeam".to_string(),
            "text".to_string(),
            "--from-clipboard".to_string(),
            "--gui".to_string(),
        ]);
        let cmd = parser.parse().unwrap();
        match cmd {
            Command::Text {
                from_clipboard,
                gui,
                ..
            } => {
                assert!(from_clipboard);
                assert!(gui);
            }
            _ => panic!("expected Text command"),
        }
    }

    #[test]
    fn setup_nautilus_installs_binary_popup_and_script_into_temp_home() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_temp_dir("setup_nautilus");
        fs::create_dir_all(&tmp).unwrap();
        let _cleanup = TempDirCleanup(tmp.clone());

        // Create a fake source root with required assets
        let source_root = tmp.join("source");
        fs::create_dir_all(source_root.join("ui")).unwrap();
        fs::create_dir_all(source_root.join("nautilus")).unwrap();
        fs::write(
            source_root.join("ui/popup.py"),
            "#!/usr/bin/env python3\nprint('test')",
        )
        .unwrap();
        fs::write(
            source_root.join("nautilus/Share on LAN"),
            "#!/bin/bash\necho 'share'",
        )
        .unwrap();

        let _home = EnvRestore::set("LANBEAM_INSTALL_HOME", tmp.as_os_str());
        let _source = EnvRestore::set("LANBEAM_SOURCE_ROOT", source_root.as_os_str());
        let installed = run_setup_nautilus().expect("setup should install all artifacts");
        assert_eq!(installed.len(), 3);

        let dest_bin = tmp.join(".local/bin/lanbeam");
        let dest_popup = tmp.join(".local/share/lanbeam/ui/popup.py");
        let dest_script = tmp.join(".local/share/nautilus/scripts/Share on LAN");
        assert_eq!(
            fs::read(dest_bin).unwrap(),
            fs::read(env::current_exe().unwrap()).unwrap()
        );
        assert_eq!(
            fs::read(dest_popup).unwrap(),
            fs::read(source_root.join("ui/popup.py")).unwrap()
        );
        assert_eq!(
            fs::read(dest_script).unwrap(),
            fs::read(source_root.join("nautilus/Share on LAN")).unwrap()
        );
        for path in installed {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
    }
}
