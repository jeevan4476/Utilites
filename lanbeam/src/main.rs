use std::process::ExitCode;

use lanbeam::{cli::Command, network, server, VERSION, qr};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let parser = lanbeam::cli::Parser::new(args);

    match parser.parse() {
        Ok(cmd) => match run(cmd) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
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
            gui: _,
            json,
        } => {
            let ip = network::detect_lan_ip();
            let preferred = port.unwrap_or(5000);
            let (listener, bound_port) = network::bind_listener(ip, preferred)
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
                println!("  URL:  {url}");
                println!("  QR:");
                let q = qr::QrCode::encode(&url)
                    .map_err(|e| format!("qr encode failed: {e}"))?;
                println!("{}", q.to_ansi());
            }

            let session = server::ServerSession::new(listener, bound_port, &token)
                .map_err(|e| format!("failed to start server: {e}"))?
                .with_mode("send")
                .with_url(&url)
                .with_share_paths(paths)
                .with_once(once)
                .with_max_downloads(max_downloads)
                .with_timeout(timeout);
            println!("  Listening on port {bound_port}. Press Ctrl+C to stop.");
            session.run().map_err(|e| format!("server error: {e}"))?;
            Ok(())
        }
        Command::Receive {
            dest,
            port,
            timeout,
            max_uploads,
            gui: _,
            json,
        } => {
            let ip = network::detect_lan_ip();
            let preferred = port.unwrap_or(5001);
            let (listener, bound_port) = network::bind_listener(ip, preferred)
                .map_err(|e| format!("failed to bind port: {e}"))?;
            let token = network::generate_token();
            let url = format!("http://{}:{}/{}", ip, bound_port, token);
            if json {
                println!("{{\"token\":\"{}\",\"ip\":\"{}\",\"port\":{}}}", token, ip, bound_port);
            } else {
                println!("lanbeam receive — listening on {url}");
            }
            let mut session = server::ServerSession::new(listener, bound_port, &token)
                .map_err(|e| format!("failed to start server: {e}"))?
                .with_mode("receive")
                .with_url(&url)
                .with_max_uploads(max_uploads)
                .with_timeout(timeout);
            if let Some(d) = dest {
                session = session.with_dest(d);
            }
            session.run().map_err(|e| format!("server error: {e}"))?;
            Ok(())
        }
        Command::Text { text, from_clipboard: _, port, timeout, once, gui: _ } => {
            let ip = network::detect_lan_ip();
            let preferred = port.unwrap_or(5002);
            let (listener, bound_port) = network::bind_listener(ip, preferred)
                .map_err(|e| format!("failed to bind port: {e}"))?;
            let token = network::generate_token();
            let url = format!("http://{}:{}/{}", ip, bound_port, token);
            println!("lanbeam text — {url}");
            let session = server::ServerSession::new(listener, bound_port, &token)
                .map_err(|e| format!("failed to start server: {e}"))?
                .with_mode("text")
                .with_url(&url)
                .with_shared_text(text)
                .with_once(once)
                .with_timeout(timeout);
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
            println!("setup-naatuilus (run install.sh to register the Nautilus script)");
            Ok(())
        }
        Command::Version => {
            println!("lanbeam {VERSION}");
            Ok(())
        }
        Command::Help => {
            println!("{}", lanbeam::cli::USAGE);
            Ok(())
        }
    }
}

#[allow(dead_code)]
const _VERSION_USE: &str = VERSION;
