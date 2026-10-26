mod common;

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use common::tmdb::{local_client, serve_once};
use common::{movie, Spec};
use movie_match::cache::DiskCache;
use movie_match::fetch::Fetcher;

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

#[test]
fn concurrent_writers_to_one_key_all_succeed() {
    let dir = temp_dir("concurrent");
    let cache = DiskCache::new(&dir);

    thread::scope(|s| {
        let writers: Vec<_> = (0..8)
            .map(|i| {
                let cache = &cache;
                s.spawn(move || {
                    for _ in 0..20 {
                        cache.put("details/1", &i).expect("write");
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().expect("writer thread");
        }
    });

    assert!(cache.get::<i32>("details/1", DAY).is_some());
    let leftovers: Vec<_> = fs::read_dir(dir.join("details"))
        .expect("read cache dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .filter(|name| name != "1.json")
        .collect();
    assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");
    let _ = fs::remove_dir_all(&dir);
}

/// A cache whose folder can't be created, because a file is in the way.
fn unwritable_cache(name: &str) -> (PathBuf, DiskCache) {
    let file = temp_dir(name);
    fs::write(&file, "not a folder").expect("write blocking file");
    let cache = DiskCache::new(file.join("cache"));
    (file, cache)
}

#[test]
fn failed_writes_are_errors_for_put_but_not_put_or_warn() {
    let (file, cache) = unwritable_cache("unwritable");

    assert!(cache.put("key", &1).is_err());
    cache.put_or_warn("key", &1);
    assert_eq!(cache.get::<i32>("key", DAY), None);
    let _ = fs::remove_file(&file);
}

#[tokio::test]
async fn fetched_details_survive_a_failed_cache_write() {
    let (file, cache) = unwritable_cache("fetch-unwritable");
    let expected = movie(
        "Dune",
        Spec {
            id: 438631,
            ..Spec::default()
        },
    );
    let body = serde_json::to_string(&expected).expect("encode details");
    let base = serve_once(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    let fetcher = Fetcher::new(local_client(base), Some(cache));

    let details = fetcher.details(&[expected.id]).await.expect("details");

    assert_eq!(details.len(), 1);
    assert_eq!(details[0].id, expected.id);
    assert_eq!(details[0].title, expected.title);
    let _ = fs::remove_file(&file);
}
