//! Searching, explaining, resolving and ranking, against a pre-filled cache and a
//! local stand-in server.

mod common;

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::tmdb::{local_client, serve_once, serve_once_recording, serve_with};
use common::{movie, Spec};
use movie_match::cache::DiskCache;
use movie_match::catalog::{catalog_cache_key, today, ReleaseWindows};
use movie_match::fetch::{
    details_cache_key, filmography_cache_key, Fetcher, CONCURRENT_REQUESTS,
    MAX_FILMOGRAPHY_FAILURES,
};
use movie_match::profile::RatedMovie;
use movie_match::recommend::{
    explain_movie, explain_movie_id, rank_upcoming, rank_upcoming_reporting, resolve,
    search_movies, Options, Resolved, Stage,
};
use movie_match::tmdb::{CreditedFilm, MovieDetails};

/// A fresh, empty folder for one test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "movie-match-test-recommend-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// A cache holding a catalog of just `upcoming` (with their details, undated ones
/// dated today so they count as upcoming), so loading the catalog with default
/// options makes no requests.
fn cache_with_upcoming(dir: &Path, upcoming: &[MovieDetails]) -> DiskCache {
    let options = Options::default().catalog;
    let windows = ReleaseWindows::new(today(), options.recent_months);
    let cache = DiskCache::new(dir);
    let ids: Vec<u64> = upcoming.iter().map(|m| m.id).collect();
    cache
        .put(
            &catalog_cache_key(&options),
            &serde_json::json!({
                "listed_on": windows.upcoming_from,
                "upcoming": ids, "recent": [], "reference": [] }),
        )
        .expect("write catalog");
    for m in upcoming {
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

    let movie = found.expect("matches the upcoming movie").movie;
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

#[tokio::test]
async fn search_lists_every_match_in_tmdbs_order() {
    let body = r#"{"page":1,"results":[
        {"id":438631,"title":"Dune","release_date":"2021-09-15"},
        {"id":841,"title":"Dune","release_date":"1984-12-14"},
        {"id":693134,"title":"Dune: Part Two"}
    ],"total_pages":1,"total_results":3}"#;
    let (base, request) = serve_once_recording(json_response(body));
    let fetcher = Fetcher::new(local_client(base), None);

    let found = search_movies(&fetcher, "  Dune \n").await.expect("search");

    let ids: Vec<u64> = found.iter().map(|m| m.id).collect();
    assert_eq!(ids, [438631, 841, 693134]);
    assert_eq!(found[1].release_date, "1984-12-14");
    assert_eq!(found[2].release_date, "", "a missing date is empty");
    let request = request
        .recv_timeout(Duration::from_secs(5))
        .expect("server saw the search request");
    assert_eq!(query_param(&request), "Dune");
}

#[tokio::test]
async fn explaining_by_id_scores_that_movie_even_outside_the_catalog() {
    let dir = temp_dir("explain-id");
    let cache = cache_with_upcoming(&dir, &[]);
    let old = MovieDetails {
        release_date: "1984-12-14".into(),
        ..movie(
            "Dune",
            Spec {
                id: 841,
                ..Spec::default()
            },
        )
    };
    cache
        .put(&details_cache_key(old.id), &old)
        .expect("write details");
    // Nothing listens here, so any request would fail the test.
    let dead = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("reserve loopback port");
    let fetcher = Fetcher::new(local_client(format!("http://{dead}")), Some(cache));

    let found = explain_movie_id(&fetcher, &[], &Options::default(), 841)
        .await
        .expect("explain");

    let movie = found.expect("explains the cached movie").movie;
    assert_eq!(movie.id, 841);
    assert_eq!(movie.release_date, "1984-12-14");
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_movie_you_rated_is_predicted_from_your_other_ratings() {
    let dir = temp_dir("explain-rated");
    let cache = cache_with_upcoming(&dir, &[]);
    for (id, title) in [(841, "Dune"), (900, "Other")] {
        let details = movie(
            title,
            Spec {
                id,
                genres: vec![878],
                ..Spec::default()
            },
        );
        cache
            .put(&details_cache_key(id), &details)
            .expect("write details");
    }
    // Nothing listens here, so any request would fail the test.
    let dead = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("reserve loopback port");
    let fetcher = Fetcher::new(local_client(format!("http://{dead}")), Some(cache));
    let rated = |id, rating| RatedMovie {
        tmdb_id: Some(id),
        rating: Some(rating),
        ..RatedMovie::default()
    };

    let found = explain_movie_id(
        &fetcher,
        &[rated(841, 10.0), rated(900, 4.0)],
        &Options::default(),
        841,
    )
    .await
    .expect("explain")
    .expect("explains the rated movie");

    assert_eq!(found.your_rating, Some(10.0));
    let neighbours: Vec<&str> = found
        .explanation
        .neighbours
        .iter()
        .map(|n| n.title.as_str())
        .collect();
    assert_eq!(neighbours, ["Other"], "not its own neighbour");
    assert_eq!(found.explanation.mean_rating, 4.0, "its rating left out");
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn explaining_an_id_tmdb_does_not_have_is_none() {
    let dir = temp_dir("explain-missing-id");
    let cache = cache_with_upcoming(&dir, &[]);
    let base =
        serve_once("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let fetcher = Fetcher::new(local_client(base), Some(cache));

    let found = explain_movie_id(&fetcher, &[], &Options::default(), 99)
        .await
        .expect("a 404 is no movie, not an error");

    assert!(found.is_none());
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

/// A `200 OK` JSON response that closes the connection.
fn json_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn ids_line_up_with_the_inputs_one_to_one() {
    let dir = temp_dir("ids-order");
    let cache = DiskCache::new(&dir);
    for id in [10, 20] {
        let details = movie(
            &format!("Movie {id}"),
            Spec {
                id,
                ..Spec::default()
            },
        );
        cache
            .put(&details_cache_key(id), &details)
            .expect("write details");
    }
    // Searches for "Found" hit movie 20 and every other search misses. Id 30 isn't
    // cached, so its details are fetched, and TMDB doesn't have it.
    let base = serve_with(|request| {
        if !request.starts_with("GET /search/movie?") {
            return "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_string();
        }
        if query_param(request) == "Found" {
            json_response(
                r#"{"page":1,"results":[{"id":20,"title":"Movie 20","release_date":"2020-01-01"}],"total_pages":1,"total_results":1}"#,
            )
        } else {
            json_response(r#"{"page":1,"results":[],"total_pages":1,"total_results":0}"#)
        }
    });
    let fetcher = Fetcher::new(local_client(base), Some(cache));
    let rated = |tmdb_id: Option<u64>, title: Option<&str>, rating: f32| RatedMovie {
        tmdb_id,
        title: title.map(String::from),
        rating: Some(rating),
        ..RatedMovie::default()
    };
    let movies = [
        rated(None, Some("Missing"), 3.0),
        rated(Some(10), Some("Saved"), 8.0),
        rated(None, None, 5.0),
        rated(None, Some("Found"), 7.0),
        rated(Some(30), Some("Gone"), 6.0),
        rated(Some(10), Some("Saved again"), 2.0),
        rated(None, Some("Found"), 4.0),
        rated(Some(20), None, 9.0),
        rated(None, Some("Also missing"), 1.0),
    ];

    let resolved = resolve(&fetcher, &movies).await;
    let _ = fs::remove_dir_all(&dir);
    let resolved = resolved.expect("resolve");

    // One id per input, in input order: a search miss, an entry with neither id nor
    // title, and an id TMDB doesn't have are `None`; duplicates get their id again.
    assert_eq!(
        resolved.ids,
        [
            None,
            Some(10),
            None,
            Some(20),
            None,
            Some(10),
            Some(20),
            Some(20),
            None,
        ]
    );
    // Duplicates keep the first rating.
    let rated: Vec<_> = resolved
        .rated
        .iter()
        .map(|r| (r.movie.id, r.rating))
        .collect();
    assert_eq!(rated, [(10, 8.0), (20, 7.0)]);
    assert_eq!(
        resolved.unmatched,
        ["Missing", "untitled entry", "Also missing", "Gone"]
    );
}

const SERVER_ERROR: &str = "HTTP/1.1 500 Internal Server Error
Content-Length: 0
Connection: close

";

#[tokio::test]
async fn a_failed_filmography_leaves_that_track_record_out_instead_of_failing() {
    let dir = temp_dir("filmography-500");
    // Directed by 7, whose films are cached, and 8, whose request fails.
    let upcoming = movie(
        "Upcoming",
        Spec {
            id: 438631,
            directors: vec![7, 8],
            ..Spec::default()
        },
    );
    let cache = cache_with_upcoming(&dir, std::slice::from_ref(&upcoming));
    let earlier = CreditedFilm {
        id: 1,
        release_date: Some("2000-01-01".into()),
        vote_average: Some(8.0),
        vote_count: Some(1000),
    };
    cache
        .put(&filmography_cache_key(7), &vec![earlier])
        .expect("write filmography");
    let fetcher = Fetcher::new(
        local_client(serve_with(|_| SERVER_ERROR.to_string())),
        Some(cache),
    );

    let ranking = rank_upcoming(&fetcher, &[], &Options::default()).await;
    let _ = fs::remove_dir_all(&dir);

    let ranking = ranking.expect("a 500 for one person doesn't fail the ranking");
    let [ranked] = ranking.ranked.as_slice() else {
        panic!("the one upcoming movie");
    };
    // Scored from director 7's record alone.
    let track_record = ranked.breakdown.track_record.as_ref().expect("director 7");
    assert!(
        (track_record.estimate - 8.0).abs() < 1e-4,
        "{track_record:?}"
    );
}

#[tokio::test]
async fn filmography_requests_stop_after_repeated_failures() {
    let dir = temp_dir("filmography-cap");
    let requests = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&requests);
    let base = serve_with(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        SERVER_ERROR.to_string()
    });
    let fetcher = Fetcher::new(local_client(base), Some(DiskCache::new(&dir)));
    let people: Vec<u64> = (1..=60).collect();

    let found = fetcher.filmographies_or_skip(&people).await;
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(found.len(), people.len());
    assert!(found.iter().all(Option::is_none));
    // Only requests already under way when the limit was hit go past it.
    let made = requests.load(Ordering::SeqCst);
    assert!(
        made < MAX_FILMOGRAPHY_FAILURES + CONCURRENT_REQUESTS,
        "{made} requests"
    );
}

#[tokio::test]
async fn ranking_reports_each_stage_in_order_with_the_people_looked_up() {
    let dir = temp_dir("stages");
    // Two directors, one of whom also acts: two people to look up, not three.
    let upcoming = movie(
        "Upcoming",
        Spec {
            id: 438631,
            directors: vec![7, 8],
            cast: vec![7],
            ..Spec::default()
        },
    );
    let cache = cache_with_upcoming(&dir, std::slice::from_ref(&upcoming));
    let earlier = CreditedFilm {
        id: 1,
        release_date: Some("2000-01-01".into()),
        vote_average: Some(7.0),
        vote_count: Some(1000),
    };
    for person in [7, 8] {
        cache
            .put(&filmography_cache_key(person), &vec![earlier.clone()])
            .expect("write filmography");
    }
    // Everything is cached, so any request reaching this server is a failure.
    let fetcher = Fetcher::new(
        local_client(serve_with(|_| SERVER_ERROR.to_string())),
        Some(cache),
    );
    let stages = Mutex::new(Vec::new());

    let ranking = rank_upcoming_reporting(&fetcher, &[], &Options::default(), |stage| {
        if let Ok(mut stages) = stages.lock() {
            stages.push(stage);
        }
    })
    .await;
    let _ = fs::remove_dir_all(&dir);

    ranking.expect("ranks from the cache");
    let stages = stages.into_inner().expect("no panics while reporting");
    assert_eq!(
        stages,
        [
            Stage::Catalog,
            Stage::Ratings,
            Stage::TrackRecords { people: 2 },
            Stage::Scoring,
        ]
    );
}
