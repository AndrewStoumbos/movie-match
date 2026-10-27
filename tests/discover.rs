//! Country codes and the `/discover/movie` query, against a local stand-in server.

mod common;

use std::time::Duration;

use common::tmdb::{local_client, serve_once_recording};
use movie_match::catalog::{catalog_cache_key, CatalogOptions};
use movie_match::tmdb::{Discover, Region};

#[test]
fn regions_are_two_letter_codes_in_upper_case() {
    assert_eq!(
        Region::parse("us").map(|r| r.to_string()),
        Some("US".into())
    );
    assert_eq!(
        Region::parse(" GB\n").map(|r| r.to_string()),
        Some("GB".into())
    );
    for bad in ["", "U", "USA", "U1", "é"] {
        assert_eq!(Region::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn locales_give_their_country() {
    let region = |locale: &str| Region::from_locale(locale).map(|r| r.to_string());
    assert_eq!(region("en-US"), Some("US".into()));
    assert_eq!(region("en_GB.UTF-8"), Some("GB".into()));
    assert_eq!(region("zh-Hans-CN"), Some("CN".into()));
    assert_eq!(region("en"), None);
    assert_eq!(region(""), None);
}

#[test]
fn the_region_is_part_of_the_catalog_key() {
    let everywhere = CatalogOptions::default();
    let us = CatalogOptions {
        upcoming_region: Region::parse("US"),
        ..everywhere
    };
    assert_ne!(catalog_cache_key(&everywhere), catalog_cache_key(&us));
}

/// The request line `discover` sends for `query`.
async fn discover_request(query: Discover<'_>) -> String {
    let body = r#"{"page":1,"results":[],"total_pages":1,"total_results":0}"#;
    let (base, request) = serve_once_recording(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    local_client(base)
        .discover(query, 1)
        .await
        .expect("discover");
    let request = request
        .recv_timeout(Duration::from_secs(5))
        .expect("server saw the request");
    request.lines().next().unwrap_or_default().to_string()
}

#[tokio::test]
async fn theatrical_region_asks_for_cinema_releases_there() {
    let line = discover_request(Discover {
        from: Some("2026-10-26"),
        theatrical_in: Region::parse("gb"),
        ..Discover::default()
    })
    .await;
    assert!(line.starts_with("GET /discover/movie?"), "{line}");
    assert!(
        line.contains("primary_release_date.gte=2026-10-26"),
        "{line}"
    );
    assert!(line.contains("region=GB"), "{line}");
    // The cinema release there must be upcoming too, not just the first release.
    assert!(line.contains("&release_date.gte=2026-10-26"), "{line}");
    // Release types 2|3: limited or wide theatrical.
    assert!(line.contains("with_release_type=2%7C3"), "{line}");
    assert!(!line.contains("vote_count"), "{line}");
}

#[tokio::test]
async fn without_a_region_every_release_counts() {
    let line = discover_request(Discover {
        from: Some("2025-10-26"),
        to: Some("2026-10-25"),
        min_votes: 50,
        ..Discover::default()
    })
    .await;
    assert!(line.contains("vote_count.gte=50"), "{line}");
    assert!(
        line.contains("primary_release_date.lte=2026-10-25"),
        "{line}"
    );
    assert!(!line.contains("region"), "{line}");
    assert!(!line.contains("with_release_type"), "{line}");
    assert!(!line.contains("&release_date"), "{line}");
}
