mod config;
mod search;
mod storage;
mod ui;
mod watcher;

use config::Config;
use storage::ClipHistory;
use ui::ClipWindow;
use watcher::ClipboardWatcher;

use glib::clone;
use gtk4::gio;
use gtk4::prelude::*;
use gtk4::Application;
use std::cell::RefCell;
use std::env;
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

pub const APP_ID: &str = "io.github.rclip";
pub const APP_PATH: &str = "/io/github/rclip";

fn main() -> glib::ExitCode {
    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("toggle");

    match cmd {
        "--version" | "-v" => {
            println!("rclip v0.1.0");
            glib::ExitCode::SUCCESS
        }
        "--help" | "-h" => {
            print_help();
            glib::ExitCode::SUCCESS
        }
        "daemon" | "--daemon" => run_daemon_service(),
        "toggle" | "show" | "hide" | "clear" => run_client_action(cmd, args.get(2).map(|s| s.as_str())),
        "add-clip" => {
            if let Some(text) = args.get(2) {
                run_client_action("add-clip", Some(text))
            } else {
                eprintln!("Usage: rclip add-clip <TEXT>");
                glib::ExitCode::FAILURE
            }
        }
        other => {
            eprintln!("Unknown command: {}", other);
            print_help();
            glib::ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "Usage: rclip [COMMAND]\n\n\
        Commands:\n  \
          toggle     Toggle clipboard popup (default)\n  \
          show       Open clipboard popup\n  \
          hide       Close clipboard popup\n  \
          clear      Clear unpinned history\n  \
          daemon     Run background daemon\n  \
          add-clip   Add text directly to history\n  \
          -v         Print version\n  \
          -h         Print help"
    );
}

fn invoke_dbus_action(action: &str, param: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let group = gio::DBusActionGroup::get(&bus, Some(APP_ID), APP_PATH);
    let variant = param.map(|p| p.to_variant());
    group.activate_action(action, variant.as_ref());
    bus.flush_sync(gio::Cancellable::NONE)?;
    Ok(())
}

fn run_client_action(action: &str, param: Option<&str>) -> glib::ExitCode {
    if invoke_dbus_action(action, param).is_ok() {
        return glib::ExitCode::SUCCESS;
    }

    // If daemon is not responding, auto-spawn daemon and retry
    if let Ok(exe) = env::current_exe() {
        let _ = Command::new(exe)
            .arg("daemon")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        for _ in 0..15 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if invoke_dbus_action(action, param).is_ok() {
                return glib::ExitCode::SUCCESS;
            }
        }
    }
    eprintln!("[rclip] Failed to invoke action '{}'", action);
    glib::ExitCode::FAILURE
}

fn run_daemon_service() -> glib::ExitCode {
    let app = Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::IS_SERVICE)
        .build();

    let cfg_path = Config::default_path();
    let config = Arc::new(Mutex::new(Config::load_or_create(&cfg_path)));

    let hist_path = ClipHistory::default_storage_path();
    let history = Arc::new(Mutex::new(ClipHistory::load_or_create(&hist_path)));

    let window_cell: Rc<RefCell<Option<Arc<ClipWindow>>>> = Rc::new(RefCell::new(None));

    // Keep app alive in background without windows
    app.connect_startup(clone!(
        #[strong] window_cell,
        #[strong] history,
        #[strong] config,
        move |app| {
            let _ = app.hold();

            let hist_watcher = history.clone();
            let cfg_watcher = config.clone();
            let win_watcher = window_cell.clone();

            ClipboardWatcher::start(hist_watcher, cfg_watcher, move || {
                if let Some(ref win) = *win_watcher.borrow() {
                    let w = win.clone();
                    glib::idle_add_local(move || {
                        w.refresh();
                        glib::ControlFlow::Break
                    });
                }
            });

            // Action: toggle
            let win_toggle = window_cell.clone();
            let hist_toggle = history.clone();
            let app_toggle = app.clone();
            let act_toggle = gio::SimpleAction::new("toggle", None);
            act_toggle.connect_activate(move |_, _| {
                let mut cell = win_toggle.borrow_mut();
                if cell.is_none() {
                    *cell = Some(Arc::new(ClipWindow::new(&app_toggle, hist_toggle.clone())));
                }
                if let Some(ref win) = *cell {
                    win.toggle();
                }
            });
            app.add_action(&act_toggle);

            // Action: show
            let win_show = window_cell.clone();
            let hist_show = history.clone();
            let app_show = app.clone();
            let act_show = gio::SimpleAction::new("show", None);
            act_show.connect_activate(move |_, _| {
                let mut cell = win_show.borrow_mut();
                if cell.is_none() {
                    *cell = Some(Arc::new(ClipWindow::new(&app_show, hist_show.clone())));
                }
                if let Some(ref win) = *cell {
                    win.show();
                }
            });
            app.add_action(&act_show);

            // Action: hide
            let win_hide = window_cell.clone();
            let act_hide = gio::SimpleAction::new("hide", None);
            act_hide.connect_activate(move |_, _| {
                if let Some(ref win) = *win_hide.borrow() {
                    win.hide();
                }
            });
            app.add_action(&act_hide);

            // Action: clear
            let hist_clear = history.clone();
            let win_clear = window_cell.clone();
            let act_clear = gio::SimpleAction::new("clear", None);
            act_clear.connect_activate(move |_, _| {
                let mut h = hist_clear.lock().unwrap();
                h.clear();
                let path = ClipHistory::default_storage_path();
                let _ = h.save_to(&path);
                drop(h);
                if let Some(ref win) = *win_clear.borrow() {
                    win.refresh();
                }
            });
            app.add_action(&act_clear);

            // Action: add-clip (used by GNOME Shell extension or external scripts)
            let hist_add = history.clone();
            let cfg_add = config.clone();
            let win_add = window_cell.clone();
            let act_add = gio::SimpleAction::new("add-clip", Some(glib::VariantTy::STRING));
            act_add.connect_activate(move |_, param| {
                if let Some(text_var) = param {
                    if let Some(text) = text_var.str() {
                        if !ClipboardWatcher::check_self_copy(text) && !text.trim().is_empty() {
                            let (max_bytes, max_items) = {
                                let cfg = cfg_add.lock().unwrap();
                                (cfg.max_clip_bytes, cfg.max_items)
                            };
                            let mut h = hist_add.lock().unwrap();
                            h.add(text, max_bytes, max_items);
                            let path = ClipHistory::default_storage_path();
                            let _ = h.save_to(&path);
                            drop(h);
                            if let Some(ref win) = *win_add.borrow() {
                                win.refresh();
                            }
                        }
                    }
                }
            });
            app.add_action(&act_add);
        }
    ));

    app.connect_activate(|_| {});
    app.run_with_args(&["rclip-daemon"])
}
