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
            timeout: _,
            once: _,
            max_downloads: _,
            gui: _,
            json,
        } => {
            let ip = network::detect_lan_ip();
            let preferred = port.unwrap_or(5000);
            let (listener, bound_port) = network::bind_listener(ip, preferred)
                .map_err(|e| format!("failed to bind port: {e}"))?;
            let token = network::generate_token();

            if json {
                println!(
                    "{{\"token\":\"{}\",\"ip\":\"{}\",\"port\":{}}}",
                    token, ip, bound_port
                );
            } else {
                println!("lanbeam send — sharing {} path(s)", paths.len());
                println!("  URL:  http://{}:{}/{}", ip, bound_port, token);
                println!("  QR:");
                let q = qr::QrCode::encode(&format!("http://{}:{}/{}", ip, bound_port, token))
                    .map_err(|e| format!("qr encode failed: {e}"))?;
                println!("{}", q.to_ansi());
            }

            let session = server::ServerSession::new(listener, bound_port, &token)
                .map_err(|e| format!("failed to start server: {e}"))?;
            println!("  Listening on port {bound_port}. Press Ctrl+C to stop.");
            session.run().map_err(|e| format!("server error: {e}"))?;
            Ok(())
        }
        Command::Receive { dest: _, port, timeout: _, max_uploads: _, gui: _, json } => {
            let ip = network::detect_lan_ip();
            let preferred = port.unwrap_or(5001);
            let (listener, bound_port) = network::bind_listener(ip, preferred)
                .map_err(|e| format!("failed to bind port: {e}"))?;
            let token = network::generate_token();
            if json {
                println!("{{\"token\":\"{}\",\"ip\":\"{}\",\"port\":{}}}", token, ip, bound_port);
            } else {
                println!("lanbeam receive — listening on http://{}:{}/{}", ip, bound_port, token);
            }
            let session = server::ServerSession::new(listener, bound_port, &token)
                .map_err(|e| format!("failed to start server: {e}"))?;
            session.run().map_err(|e| format!("server error: {e}"))?;
            Ok(())
        }
        Command::Text { text: _, from_clipboard: _, port, timeout: _, once: _, gui: _ } => {
            let ip = network::detect_lan_ip();
            let preferred = port.unwrap_or(5002);
            let (listener, bound_port) = network::bind_listener(ip, preferred)
                .map_err(|e| format!("failed to bind port: {e}"))?;
            let token = network::generate_token();
            println!("lanbeam text — http://{}:{}/{}", ip, bound_port, token);
            let session = server::ServerSession::new(listener, bound_port, &token)
                .map_err(|e| format!("failed to start server: {e}"))?;
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
    }
}

#[allow(dead_code)]
const _VERSION_USE: &str = VERSION;
