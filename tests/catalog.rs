//! Loading the catalog from a cached id list when some ids no longer load.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::tmdb::{local_client, serve_once, serve_with};
use common::{movie, Spec};
use movie_match::cache::DiskCache;
use movie_match::catalog::{
    catalog_cache_key, today, Catalog, CatalogOptions, ReleaseWindows, CATALOG_MAX_AGE,
};
use movie_match::fetch::{details_cache_key, Fetcher};
use movie_match::tmdb::MovieDetails;

const NOT_FOUND: &str = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

/// A fresh, empty folder for one test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "movie-match-test-catalog-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// A cache whose catalog lists `upcoming_ids` and no recent or reference movies,
/// with details saved only for `cached` (undated ones dated today, so they count
/// as upcoming).
fn cache_with_catalog(dir: &Path, upcoming_ids: &[u64], cached: &[MovieDetails]) -> DiskCache {
    let options = CatalogOptions::default();
    let windows = ReleaseWindows::new(today(), options.recent_months);
    let cache = DiskCache::new(dir);
    cache
        .put(
            &catalog_cache_key(&options),
            &serde_json::json!({
                "listed_on": windows.upcoming_from,
                "upcoming": upcoming_ids, "recent": [], "reference": [] }),
        )
        .expect("write catalog");
    for m in cached {
        let mut m = m.clone();
        if m.release_date.is_empty() {
            m.release_date = windows.upcoming_from.clone();
        }
        cache
            .put(&details_cache_key(m.id), &m)
            .expect("write details");
    }
    cache
}

#[tokio::test]
async fn a_catalog_listed_on_another_day_is_listed_again() {
    let dir = temp_dir("other-day");
    let options = CatalogOptions::default();
    let key = catalog_cache_key(&options);
    let cache = cache_with_catalog(&dir, &[1], &[]);
    cache
        .put(
            &key,
            &serde_json::json!({
                "listed_on": "2000-01-01",
                "upcoming": [1], "recent": [], "reference": [] }),
        )
        .expect("write yesterday's catalog");
    // Every list TMDB returns now is empty, and movie 1's details aren't cached,
    // so loading the old list would fail.
    let empty = r#"{"page":1,"results":[],"total_pages":1,"total_results":0}"#;
    let base = serve_with(move |_| {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{empty}",
            empty.len()
        )
    });
    let fetcher = Fetcher::new(local_client(base), Some(DiskCache::new(&dir)));

    let catalog = Catalog::load(&fetcher, options)
        .await
        .expect("catalog is listed again");

    assert!(catalog.upcoming.is_empty());
    // The one file for these options now holds today's lists.
    let saved: serde_json::Value = DiskCache::new(&dir)
        .get(&key, CATALOG_MAX_AGE)
        .expect("catalog saved");
    let today = ReleaseWindows::new(today(), options.recent_months).upcoming_from;
    assert_eq!(saved["listed_on"], today);
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn removed_movie_is_skipped_instead_of_failing_the_catalog() {
    let dir = temp_dir("removed");
    let kept = movie(
        "Kept",
        Spec {
            id: 1,
            ..Spec::default()
        },
    );
    let removed_id = 2;
    let cache = cache_with_catalog(&dir, &[kept.id, removed_id], std::slice::from_ref(&kept));
    // The one request made is for the removed movie, which TMDB no longer has.
    let fetcher = Fetcher::new(local_client(serve_once(NOT_FOUND)), Some(cache));

    let catalog = Catalog::load(&fetcher, CatalogOptions::default())
        .await
        .expect("catalog loads without the removed movie");

    let ids: Vec<u64> = catalog.upcoming.iter().map(|m| m.id).collect();
    assert_eq!(ids, [kept.id]);
    assert!(catalog.reference.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn catalog_fails_when_no_movie_loads() {
    let dir = temp_dir("none");
    let cache = cache_with_catalog(&dir, &[2], &[]);
    let fetcher = Fetcher::new(local_client(serve_once(NOT_FOUND)), Some(cache));

    let err = Catalog::load(&fetcher, CatalogOptions::default())
        .await
        .expect_err("no upcoming movie loads");

    let message = format!("{err:#}");
    assert!(message.contains("TMDB movie 2"), "{message}");
    assert!(message.contains("404"), "{message}");
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn transient_error_fails_the_catalog_instead_of_skipping_the_movie() {
    let dir = temp_dir("transient");
    let kept = movie(
        "Kept",
        Spec {
            id: 1,
            ..Spec::default()
        },
    );
    let cache = cache_with_catalog(&dir, &[kept.id, 2], std::slice::from_ref(&kept));
    let unavailable =
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let fetcher = Fetcher::new(local_client(serve_once(unavailable)), Some(cache));

    let err = Catalog::load(&fetcher, CatalogOptions::default())
        .await
        .expect_err("a 503 is not a removed movie");

    let message = format!("{err:#}");
    assert!(message.contains("TMDB movie 2"), "{message}");
    assert!(message.contains("503"), "{message}");
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn undecodable_details_fail_the_catalog_instead_of_skipping_the_movie() {
    let dir = temp_dir("undecodable");
    let kept = movie(
        "Kept",
        Spec {
            id: 1,
            ..Spec::default()
        },
    );
    let cache = cache_with_catalog(&dir, &[kept.id, 2], std::slice::from_ref(&kept));
    let garbage = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot json";
    let fetcher = Fetcher::new(local_client(serve_once(garbage)), Some(cache));

    let err = Catalog::load(&fetcher, CatalogOptions::default())
        .await
        .expect_err("a bad body is not a removed movie");

    let message = format!("{err:#}");
    assert!(message.contains("could not decode"), "{message}");
    let _ = fs::remove_dir_all(&dir);
}
