//! File logging: a daily log file (`<file>.YYYY-MM-DD`) that is set up
//! without panicking (#234) and pruned to the last `logging.max_files`
//! files (#240).

use std::path::{Path, PathBuf};
use tracing_appender::rolling::{RollingFileAppender, Rotation};

/// Where a daily log file lives: its directory and the file name every
/// day's file starts with (`harux.log` → `harux.log.2026-11-11`).
#[derive(Debug, Clone)]
pub struct LogTarget {
    pub dir: PathBuf,
    pub name: String,
}

impl LogTarget {
    /// Split `logging.file` into directory and file name. A path without a
    /// directory part logs to the working directory.
    pub fn from_config(file: &str) -> Self {
        let path = Path::new(file);
        let dir = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("harux.log")
            .to_string();
        Self { dir, name }
    }

    /// Open today's file (creating the directory if needed). Unlike
    /// `rolling::daily`, a directory that can't be created or a file that
    /// can't be opened is an error, not a panic (#234).
    pub fn appender(&self) -> Result<RollingFileAppender, String> {
        RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix(&self.name)
            .build(&self.dir)
            .map_err(|e| e.to_string())
    }

    /// Whether `file_name` is one of this target's daily files: exactly
    /// `<name>.YYYY-MM-DD`. Anything else in the directory — the binary,
    /// `harux.toml`, `harux.log.bak` — never matches, whatever `name` is.
    fn is_daily_file(&self, file_name: &str) -> bool {
        let Some(date) = file_name
            .strip_prefix(self.name.as_str())
            .and_then(|rest| rest.strip_prefix('.'))
        else {
            return false;
        };
        let b = date.as_bytes();
        b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b.iter()
                .enumerate()
                .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    }

    /// Delete all but the newest `keep` daily files (today's included) and
    /// return the deleted paths. `keep == 0` keeps everything. Dates sort
    /// as text, so the newest are the last by name; file times are not
    /// used, so copying the directory doesn't change what is kept.
    pub fn prune(&self, keep: usize) -> std::io::Result<Vec<PathBuf>> {
        if keep == 0 {
            return Ok(Vec::new());
        }
        let mut daily: Vec<PathBuf> = std::fs::read_dir(&self.dir)?
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| self.is_daily_file(n))
            })
            .map(|e| e.path())
            .collect();
        if daily.len() <= keep {
            return Ok(Vec::new());
        }
        daily.sort();
        let excess = daily.len() - keep;
        let mut deleted = Vec::with_capacity(excess);
        for path in daily.into_iter().take(excess) {
            std::fs::remove_file(&path)?;
            deleted.push(path);
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("harux-logging-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn target_splits_the_configured_path() {
        let t = LogTarget::from_config("logs/harux.log");
        assert_eq!(t.dir, Path::new("logs"));
        assert_eq!(t.name, "harux.log");
        let t = LogTarget::from_config("harux.log");
        assert_eq!(t.dir, Path::new("."));
    }

    /// #240: only the newest `keep` daily files survive, and nothing that
    /// merely starts with the log name is touched — even when the log is
    /// named like the binary.
    #[test]
    fn prune_keeps_the_newest_daily_files_only() {
        let dir = temp_dir("prune");
        let t = LogTarget {
            dir: dir.clone(),
            name: "harux".into(),
        };
        let daily = [
            "harux.2026-10-30",
            "harux.2026-10-31",
            "harux.2026-11-01",
            "harux.2026-11-10",
            "harux.2026-11-11",
        ];
        let others = [
            "harux",
            "harux.exe",
            "harux.toml",
            "harux.log",
            "harux.2026-11-1",
            "harux.2026-11-01.bak",
            "harux.20261101xx",
            "other.2026-01-01",
        ];
        for f in daily.iter().chain(&others) {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        std::fs::create_dir(dir.join("harux.2020-01-01")).unwrap(); // a directory

        assert!(t.prune(0).unwrap().is_empty(), "0 keeps everything");
        assert!(t.prune(5).unwrap().is_empty());

        let mut deleted: Vec<String> = t
            .prune(2)
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        deleted.sort();
        assert_eq!(deleted, &daily[..3]);
        for f in daily[3..].iter().chain(&others) {
            assert!(dir.join(f).is_file(), "{f} was deleted");
        }
        assert!(dir.join("harux.2020-01-01").is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #234: a directory that can't be created is an error, not a panic.
    #[test]
    fn unusable_log_directory_is_an_error() {
        let dir = temp_dir("blocked");
        // A regular file where the log directory should be.
        std::fs::write(dir.join("blocker"), "x").unwrap();
        let t = LogTarget::from_config(dir.join("blocker").join("harux.log").to_str().unwrap());
        let err = t.appender().unwrap_err();
        assert!(!err.is_empty());
        // A usable directory works and creates today's file.
        let ok = LogTarget::from_config(dir.join("logs").join("harux.log").to_str().unwrap());
        drop(ok.appender().unwrap());
        let created: Vec<_> = std::fs::read_dir(dir.join("logs")).unwrap().collect();
        assert_eq!(created.len(), 1);
        let name = created[0].as_ref().unwrap().file_name();
        assert!(ok.is_daily_file(name.to_str().unwrap()), "{name:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
