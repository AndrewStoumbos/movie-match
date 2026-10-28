//! The server against a stand-in TMDB, used through the library's own client.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use movie_match::cache::DiskCache;
use movie_match::tmdb::{is_not_found, TmdbClient};
use movie_match_server::{router, Server};

const SERVER_KEY: &str = "server-test-key";

/// What the stand-in TMDB saw: one query string per request.
#[derive(Default)]
struct Seen {
    queries: Mutex<Vec<String>>,
    requests: AtomicUsize,
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, app).await });
    addr
}

/// A stand-in TMDB with movie 7 and nothing else, and a Movie Match server in front
/// of it caching in `cache_dir`. Returns the server's URL and what TMDB saw.
async fn start(cache_dir: Option<&std::path::Path>) -> (String, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let tmdb = Router::new()
        .route(
            "/movie/{id}",
            get(
                |State(seen): State<Arc<Seen>>, Path(id): Path<u64>, RawQuery(q): RawQuery| async move {
                    seen.requests.fetch_add(1, Ordering::SeqCst);
                    seen.queries.lock().unwrap().push(q.unwrap_or_default());
                    if id == 7 {
                        (StatusCode::OK, r#"{"id":7,"title":"Seven"}"#).into_response()
                    } else {
                        StatusCode::NOT_FOUND.into_response()
                    }
                },
            ),
        )
        .with_state(Arc::clone(&seen));
    let tmdb_addr = serve(tmdb).await;
    let server = Server::new(
        SERVER_KEY.to_string(),
        format!("http://{tmdb_addr}"),
        cache_dir.map(DiskCache::new),
    )
    .expect("server");
    let addr = serve(router(Arc::new(server))).await;
    (format!("http://{addr}"), seen)
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("HTTP client")
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("movie-match-server-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[tokio::test]
async fn the_app_gets_movies_without_a_key_and_repeats_come_from_the_cache() {
    let dir = temp_dir("cache");
    let (url, seen) = start(Some(&dir)).await;
    let client = TmdbClient::via_server_using(http(), &url);

    let first = client
        .movie_details(7)
        .await
        .expect("details through the server");
    let again = client.movie_details(7).await.expect("details again");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        (first.title.as_str(), again.title.as_str()),
        ("Seven", "Seven")
    );
    assert_eq!(
        seen.requests.load(Ordering::SeqCst),
        1,
        "the repeat was cached"
    );
    let queries = seen.queries.lock().unwrap();
    assert!(
        queries[0].contains(&format!("api_key={SERVER_KEY}")),
        "{queries:?}"
    );
    assert!(queries[0].contains("append_to_response="), "{queries:?}");
}

#[tokio::test]
async fn tmdb_404s_pass_through_uncached() {
    let dir = temp_dir("not-found");
    let (url, seen) = start(Some(&dir)).await;
    let client = TmdbClient::via_server_using(http(), &url);

    for _ in 0..2 {
        let err = client.movie_details(404).await.expect_err("no such movie");
        assert!(is_not_found(&err), "{err:#}");
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(seen.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn requests_the_app_never_makes_are_refused() {
    let (url, seen) = start(None).await;

    for path in [
        "/tmdb/account/1",
        "/tmdb/movie/upcoming",
        "/tmdb/movie/7/videos",
        "/tmdb/movie/abc",
        "/other",
    ] {
        let status = http()
            .get(format!("{url}{path}"))
            .send()
            .await
            .expect("request")
            .status();
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(seen.requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_key_sent_to_the_server_is_replaced_by_its_own() {
    let (url, seen) = start(None).await;

    let status = http()
        .get(format!("{url}/tmdb/movie/7?api_key=someone-elses"))
        .send()
        .await
        .expect("request")
        .status();

    assert!(status.is_success());
    let queries = seen.queries.lock().unwrap();
    assert!(!queries[0].contains("someone-elses"), "{queries:?}");
    assert!(
        queries[0].contains(&format!("api_key={SERVER_KEY}")),
        "{queries:?}"
    );
}
