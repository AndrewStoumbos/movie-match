//! Loading the catalog from a cached id list when some ids no longer load.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::tmdb::{local_client, serve_once};
use common::{movie, Spec};
use movie_match::cache::DiskCache;
use movie_match::catalog::{Catalog, CatalogOptions};
use movie_match::fetch::Fetcher;
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

/// A cache whose catalog lists `upcoming_ids` and no reference movies, with
/// details saved only for `cached`.
fn cache_with_catalog(dir: &Path, upcoming_ids: &[u64], cached: &[MovieDetails]) -> DiskCache {
    let options = CatalogOptions::default();
    let cache = DiskCache::new(dir);
    cache
        .put(
            &format!(
                "catalog-u{}-r{}",
                options.upcoming_pages, options.reference_pages
            ),
            &serde_json::json!({ "upcoming": upcoming_ids, "reference": [] }),
        )
        .expect("write catalog");
    for m in cached {
        cache
            .put(&format!("details/{}", m.id), m)
            .expect("write details");
    }
    cache
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
