//! A small JSON file cache, so repeat runs don't re-download from TMDB.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Entry<T> {
    saved_at: u64,
    value: T,
}

#[derive(Serialize)]
struct EntryRef<'a, T> {
    saved_at: u64,
    value: &'a T,
}

/// Stores values as `<dir>/<key>.json`. Keys may contain `/` to group entries
/// into subfolders, e.g. `details/329865`.
#[derive(Debug, Clone)]
pub struct DiskCache {
    dir: PathBuf,
    refresh: bool,
}

impl DiskCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            refresh: false,
        }
    }

    /// A cache that ignores stored values (every read misses) but still saves new ones.
    pub fn refreshing(mut self) -> Self {
        self.refresh = true;
        self
    }

    /// The value saved under `key`, if it's at most `max_age` old. Missing,
    /// expired and unreadable entries all count as misses.
    pub fn get<T: DeserializeOwned>(&self, key: &str, max_age: Duration) -> Option<T> {
        if self.refresh {
            return None;
        }
        let text = fs::read_to_string(self.path(key)).ok()?;
        let entry: Entry<T> = serde_json::from_str(&text).ok()?;
        let age = now_secs().saturating_sub(entry.saved_at);
        (age <= max_age.as_secs()).then_some(entry.value)
    }

    pub fn put<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let entry = EntryRef {
            saved_at: now_secs(),
            value,
        };
        let json = serde_json::to_string(&entry).context("could not encode cache entry")?;
        // Write to a temporary file and rename it, so readers never see half a file.
        // The name is unique to this write, so writers sharing the cache (other
        // threads or processes) never move each other's temporary files.
        static WRITES: AtomicU64 = AtomicU64::new(0);
        let tmp = path.with_extension(format!(
            "json.{}-{}.tmp",
            std::process::id(),
            WRITES.fetch_add(1, Ordering::Relaxed)
        ));
        let written = fs::write(&tmp, json)
            .with_context(|| format!("could not write {}", tmp.display()))
            .and_then(|()| {
                fs::rename(&tmp, &path)
                    .with_context(|| format!("could not write {}", path.display()))
            });
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written
    }

    /// Like [`put`](Self::put), but a failed write only prints a warning to stderr.
    /// The cache just saves work, so a full or read-only cache shouldn't stop a run
    /// that already has the value.
    pub fn put_or_warn<T: Serialize>(&self, key: &str, value: &T) {
        if let Err(err) = self.put(key, value) {
            eprintln!("warning: could not cache {key}: {err:#}");
        }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
