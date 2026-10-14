use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClipItem {
    pub id: u64,
    pub text: String,
    pub timestamp: u64,
    pub pinned: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ClipHistory {
    pub items: Vec<ClipItem>,
    #[serde(skip)]
    next_id: u64,
}

impl ClipHistory {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
        }
    }

    pub fn default_storage_path() -> PathBuf {
        let mut path = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.trim().is_empty() {
                PathBuf::from(xdg)
            } else {
                dirs_fallback()
            }
        } else {
            dirs_fallback()
        };
        path.push("rclip");
        path.push("history.json");
        path
    }

    pub fn load_or_create(path: &Path) -> Self {
        if let Ok(file) = File::open(path) {
            let reader = BufReader::new(file);
            if let Ok(mut hist) = serde_json::from_reader::<_, ClipHistory>(reader) {
                let max_id = hist.items.iter().map(|i| i.id).max().unwrap_or(0);
                hist.next_id = max_id + 1;
                return hist;
            }
        }
        Self::new()
    }

    pub fn add(&mut self, text: &str, max_clip_bytes: usize, max_items: usize) -> bool {
        // Enforce max byte limit
        if text.len() > max_clip_bytes {
            return false;
        }

        // Ignore empty or whitespace-only clips
        if text.trim().is_empty() {
            return false;
        }

        // Check if item already exists (deduplication)
        if let Some(pos) = self.items.iter().position(|i| i.text == text) {
            let mut item = self.items.remove(pos);
            item.timestamp = current_timestamp();
            // Pinned items stay pinned, move to front
            self.items.insert(0, item);
            self.sort_pinned_first();
            return false;
        }

        let item = ClipItem {
            id: self.next_id,
            text: text.to_string(), // Preserve indentation
            timestamp: current_timestamp(),
            pinned: false,
        };
        self.next_id += 1;
        self.items.insert(0, item);
        self.sort_pinned_first();

        let max = if max_items == 0 { 100 } else { max_items };
        while self.items.len() > max {
            if let Some(last_unpinned) = self.items.iter().rposition(|i| !i.pinned) {
                self.items.remove(last_unpinned);
            } else {
                break;
            }
        }

        true
    }

    pub fn toggle_pin(&mut self, id: u64) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.pinned = !item.pinned;
            self.sort_pinned_first();
            true
        } else {
            false
        }
    }

    pub fn delete(&mut self, id: u64) -> bool {
        if let Some(pos) = self.items.iter().position(|i| i.id == id) {
            self.items.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) {
        self.items.retain(|i| i.pinned);
    }

    fn sort_pinned_first(&mut self) {
        // Stable sort: pinned items first, within each group keep timestamp descending
        self.items.sort_by(|a, b| match (b.pinned, a.pinned) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => b.timestamp.cmp(&a.timestamp),
        });
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            builder.mode(0o700);
            builder.create(parent)?;
        }

        let tmp_path = path.with_extension("json.tmp");
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp_path)?;

        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, self)?;
        writer.flush()?;

        let f = writer.into_inner()?;
        f.sync_all()?;
        drop(f);

        fs::rename(&tmp_path, path)?;
        Ok(())
    }
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn dirs_fallback() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".local").join("share")
    } else {
        PathBuf::from("/tmp")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn test_add_and_deduplicate() {
        let mut history = ClipHistory::new();
        assert!(history.add("hello", 1_000_000, 10));
        assert_eq!(history.items.len(), 1);
        assert_eq!(history.items[0].text, "hello");

        // Re-adding moves to top, does not duplicate
        assert!(!history.add("hello", 1_000_000, 10));
        assert_eq!(history.items.len(), 1);

        assert!(history.add("world", 1_000_000, 10));
        assert_eq!(history.items.len(), 2);
        assert_eq!(history.items[0].text, "world");
        assert_eq!(history.items[1].text, "hello");
    }

    #[test]
    fn test_max_clip_bytes_rejection() {
        let mut history = ClipHistory::new();
        assert!(!history.add("too long string", 5, 10));
        assert_eq!(history.items.len(), 0);
        assert!(history.add("short", 10, 10));
        assert_eq!(history.items.len(), 1);
    }

    #[test]
    fn test_pinned_sorting_and_pruning() {
        let mut history = ClipHistory::new();
        history.add("first", 1_000_000, 2);
        let id_first = history.items[0].id;
        history.toggle_pin(id_first);

        history.add("second", 1_000_000, 2);
        history.add("third", 1_000_000, 2);

        // Capacity is 2: "first" is pinned, so "second" is pruned instead of "first"
        assert_eq!(history.items.len(), 2);
        assert!(history.items[0].pinned);
        assert_eq!(history.items[0].text, "first");
        assert_eq!(history.items[1].text, "third");
    }

    #[test]
    fn test_file_permissions_and_sync() {
        let temp_dir = std::env::temp_dir().join("rclip_perm_test");
        let path = temp_dir.join("history.json");

        let mut history = ClipHistory::new();
        history.add("secret data", 1_000_000, 10);
        history.save_to(&path).unwrap();

        // Check directory permissions mode 0700
        let dir_meta = fs::metadata(&temp_dir).unwrap();
        assert_eq!(dir_meta.permissions().mode() & 0o777, 0o700);

        // Check file permissions mode 0600
        let file_meta = fs::metadata(&path).unwrap();
        assert_eq!(file_meta.permissions().mode() & 0o777, 0o600);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
