use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    pub max_items: usize,
    pub max_clip_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_items: 100,
            max_clip_bytes: 1_000_000, // 1 MB
        }
    }
}

impl Config {
    pub fn default_path() -> PathBuf {
        let mut path = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.trim().is_empty() {
                PathBuf::from(xdg)
            } else {
                dirs_config_fallback()
            }
        } else {
            dirs_config_fallback()
        };
        path.push("rclip");
        path.push("config.json");
        path
    }

    pub fn load_or_create(path: &Path) -> Self {
        if let Ok(file) = File::open(path) {
            let reader = BufReader::new(file);
            if let Ok(cfg) = serde_json::from_reader(reader) {
                return cfg;
            }
        }

        let default_cfg = Self::default();
        let _ = default_cfg.save_to(path);
        default_cfg
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, self)?;
        Ok(())
    }
}

fn dirs_config_fallback() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config")
    } else {
        PathBuf::from("/tmp")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let cfg = Config::default();
        assert_eq!(cfg.max_items, 100);
        assert_eq!(cfg.max_clip_bytes, 1_000_000);
    }

    #[test]
    fn test_config_roundtrip() {
        let temp_dir = std::env::temp_dir().join("rclip_test_cfg");
        let path = temp_dir.join("test_config.json");

        let mut cfg = Config::default();
        cfg.max_items = 50;
        cfg.max_clip_bytes = 500_000;
        cfg.save_to(&path).unwrap();

        let loaded = Config::load_or_create(&path);
        assert_eq!(loaded, cfg);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
