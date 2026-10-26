//! `explain_movie` lookups, against a pre-filled cache and a local stand-in server.

mod common;

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::tmdb::{local_client, serve_once, serve_once_recording};
use common::{movie, Spec};
use movie_match::cache::DiskCache;
use movie_match::fetch::{details_cache_key, Fetcher};
use movie_match::profile::RatedMovie;
use movie_match::recommend::{explain_movie, resolve, Options, Resolved};
use movie_match::tmdb::MovieDetails;

/// A fresh, empty folder for one test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "movie-match-test-recommend-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// A cache holding a catalog of just `upcoming` (with their details), so loading
/// the catalog with default options makes no requests.
fn cache_with_upcoming(dir: &Path, upcoming: &[MovieDetails]) -> DiskCache {
    let options = Options::default().catalog;
    let cache = DiskCache::new(dir);
    let ids: Vec<u64> = upcoming.iter().map(|m| m.id).collect();
    cache
        .put(
            &format!(
                "catalog-u{}-r{}",
                options.upcoming_pages, options.reference_pages
            ),
            &serde_json::json!({ "upcoming": ids, "reference": [] }),
        )
        .expect("write catalog");
    for m in upcoming {
        cache
            .put(&details_cache_key(m.id), m)
            .expect("write details");
    }
    cache
}

/// The decoded value of the `query` parameter in a raw HTTP request head.
fn query_param(request: &str) -> String {
    let line = request.lines().next().unwrap_or_default();
    let target = line.split(' ').nth(1).unwrap_or_default();
    let query = target.split_once('?').map_or("", |(_, q)| q);
    let value = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("query="))
        .unwrap_or_default();
    value.replace('+', " ").replace("%20", " ")
}

#[tokio::test]
async fn upcoming_match_ignores_surrounding_whitespace() {
    let dir = temp_dir("upcoming");
    let upcoming = movie(
        "Dune",
        Spec {
            id: 438631,
            ..Spec::default()
        },
    );
    let cache = cache_with_upcoming(&dir, std::slice::from_ref(&upcoming));
    // Nothing listens here, so any request would fail the test.
    let dead = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("reserve loopback port");
    let fetcher = Fetcher::new(local_client(format!("http://{dead}")), Some(cache));

    let found = explain_movie(&fetcher, &[], &Options::default(), "  dune \n")
        .await
        .expect("explain");

    let (movie, _) = found.expect("matches the upcoming movie");
    assert_eq!(movie.id, upcoming.id);
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn search_fallback_sends_trimmed_title() {
    let dir = temp_dir("search");
    let cache = cache_with_upcoming(&dir, &[]);
    let body = r#"{"page":1,"results":[],"total_pages":1,"total_results":0}"#;
    let (base, request) = serve_once_recording(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    let fetcher = Fetcher::new(local_client(base), Some(cache));

    let found = explain_movie(&fetcher, &[], &Options::default(), "  Past Lives \t")
        .await
        .expect("explain");

    assert!(found.is_none(), "search returned no results");
    let request = request
        .recv_timeout(Duration::from_secs(5))
        .expect("server saw the search request");
    assert!(request.starts_with("GET /search/movie?"), "{request}");
    assert_eq!(query_param(&request), "Past Lives");
    let _ = fs::remove_dir_all(&dir);
}

/// Two ratings with saved TMDB ids: id 1 is cached, id 2 is fetched from a stand-in
/// server that answers with `response`.
async fn resolve_known_and_fetched(name: &str, response: &str) -> anyhow::Result<Resolved> {
    let dir = temp_dir(name);
    let known = movie(
        "Known",
        Spec {
            id: 1,
            ..Spec::default()
        },
    );
    let cache = DiskCache::new(&dir);
    cache
        .put(&details_cache_key(known.id), &known)
        .expect("write details");
    let fetcher = Fetcher::new(local_client(serve_once(response)), Some(cache));
    let movies = [
        RatedMovie {
            tmdb_id: Some(1),
            title: Some("Known".into()),
            rating: Some(8.0),
            ..RatedMovie::default()
        },
        RatedMovie {
            tmdb_id: Some(2),
            title: Some("Gone".into()),
            rating: Some(9.0),
            ..RatedMovie::default()
        },
    ];
    let result = resolve(&fetcher, &movies).await;
    let _ = fs::remove_dir_all(&dir);
    result
}

#[tokio::test]
async fn a_saved_id_tmdb_does_not_have_counts_as_unmatched() {
    let resolved = resolve_known_and_fetched(
        "missing-id",
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await
    .expect("a 404 for one rating doesn't fail the others");

    assert_eq!(resolved.rated.len(), 1);
    assert_eq!(resolved.rated[0].movie.id, 1);
    assert_eq!(resolved.unmatched, ["Gone"]);
    assert_eq!(resolved.ids, [Some(1), None]);
}

#[tokio::test]
async fn other_tmdb_errors_still_fail_resolving() {
    let err = resolve_known_and_fetched(
        "server-error",
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await
    .expect_err("a server error must not quietly drop a rating");
    assert!(format!("{err:#}").contains("500"), "{err:#}");
}
