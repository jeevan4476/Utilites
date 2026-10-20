use crate::config::Config;
use crate::storage::ClipHistory;
use glib::clone;
use gtk4::gio;
use gtk4::gdk;
use gtk4::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static LAST_SELF_COPY_HASH: AtomicU64 = AtomicU64::new(0);

pub fn simple_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h = (h ^ (b as u64)).wrapping_mul(0x100000001b3);
    }
    h
}

pub struct ClipboardWatcher;

impl ClipboardWatcher {
    pub fn notify_self_copy(text: &str) {
        LAST_SELF_COPY_HASH.store(simple_hash(text), Ordering::SeqCst);
    }

    pub fn check_self_copy(text: &str) -> bool {
        let text_hash = simple_hash(text);
        if text_hash == 0 {
            false
        } else {
            LAST_SELF_COPY_HASH
                .compare_exchange(text_hash, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        }
    }

    pub fn start<F: Fn() + 'static>(
        history: Arc<Mutex<ClipHistory>>,
        config: Arc<Mutex<Config>>,
        on_update: F,
    ) {
        let display = match gdk::Display::default() {
            Some(d) => d,
            None => {
                eprintln!("[rclip] Warning: No default GDK display available for watcher.");
                return;
            }
        };

        let clipboard = display.clipboard();
        let on_update = Arc::new(on_update);

        clipboard.connect_changed(clone!(
            #[strong] history,
            #[strong] config,
            #[strong] on_update,
            move |clip| {
                // Ignore password manager copies
                let formats = clip.formats();
                if formats.contain_mime_type("x-kde-passwordManagerHint") {
                    return;
                }

                clip.read_text_async(None::<&gio::Cancellable>, clone!(
                    #[strong] history,
                    #[strong] config,
                    #[strong] on_update,
                    move |res| {
                        if let Ok(Some(text)) = res {
                            let text_str = text.as_str();

                            // Skip if event originated from rclip UI self-copy
                            if Self::check_self_copy(text_str) {
                                return;
                            }

                            if !text_str.trim().is_empty() {
                                let (max_bytes, max_items) = {
                                    let cfg = config.lock().unwrap();
                                    (cfg.max_clip_bytes, cfg.max_items)
                                };

                                let mut hist = history.lock().unwrap();
                                if hist.add(text_str, max_bytes, max_items) || true {
                                    let path = ClipHistory::default_storage_path();
                                    let _ = hist.save_to(&path);
                                    on_update();
                                }
                            }
                        }
                    }
                ));
            }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_self_copy_lifecycle() {
        assert!(!ClipboardWatcher::check_self_copy("sample text"));
        ClipboardWatcher::notify_self_copy("sample text");
        assert!(!ClipboardWatcher::check_self_copy("different text"));
        assert!(ClipboardWatcher::check_self_copy("sample text"));
        assert!(!ClipboardWatcher::check_self_copy("sample text"));
    }
}
