use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use movie_match::cache::DiskCache;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// A fresh, empty folder for one test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("movie-match-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

#[test]
fn saved_values_come_back_until_they_expire() {
    let dir = temp_dir("roundtrip");
    let cache = DiskCache::new(&dir);
    cache.put("details/42", &[1, 2, 3]).expect("write");

    assert_eq!(
        cache.get::<Vec<i32>>("details/42", DAY),
        Some(vec![1, 2, 3])
    );
    assert!(dir.join("details").join("42.json").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn old_missing_and_corrupt_entries_are_misses() {
    let dir = temp_dir("misses");
    fs::create_dir_all(&dir).expect("create dir");
    fs::write(dir.join("old.json"), r#"{"saved_at":0,"value":7}"#).expect("write");
    fs::write(dir.join("corrupt.json"), "not json").expect("write");
    let cache = DiskCache::new(&dir);

    assert_eq!(cache.get::<i32>("old", DAY), None);
    assert_eq!(cache.get::<i32>("missing", DAY), None);
    assert_eq!(cache.get::<i32>("corrupt", DAY), None);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn refreshing_cache_ignores_stored_values_but_still_saves() {
    let dir = temp_dir("refresh");
    DiskCache::new(&dir).put("key", &1).expect("write");

    let refreshing = DiskCache::new(&dir).refreshing();
    assert_eq!(refreshing.get::<i32>("key", DAY), None);
    refreshing.put("key", &2).expect("write");

    assert_eq!(DiskCache::new(&dir).get::<i32>("key", DAY), Some(2));
    let _ = fs::remove_dir_all(&dir);
}
