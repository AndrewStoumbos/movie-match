//! The server against a stand-in TMDB, used through the library's own client.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, RawQuery, State};
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

/// A stand-in TMDB with movie 7, a search that names its one result after the query,
/// and nothing else, plus a Movie Match server in front of it caching in
/// `cache_dir`. Returns the server's URL and what TMDB saw.
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
        .route(
            "/search/movie",
            get(
                |State(seen): State<Arc<Seen>>, Query(q): Query<Vec<(String, String)>>| async move {
                    seen.requests.fetch_add(1, Ordering::SeqCst);
                    let query = q
                        .iter()
                        .find(|(k, _)| k == "query")
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    let title = serde_json::to_string(&query).unwrap_or_default();
                    format!(
                        r#"{{"page":1,"results":[{{"id":1,"title":{title}}}],"total_pages":1,"total_results":1}}"#
                    )
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

/// The status the server answers for `path_and_query`.
async fn status(url: &str, path_and_query: &str) -> reqwest::StatusCode {
    http()
        .get(format!("{url}{path_and_query}"))
        .send()
        .await
        .expect("request")
        .status()
}

#[tokio::test]
async fn requests_the_app_never_makes_are_refused() {
    let (url, seen) = start(None).await;

    for refused in [
        // Paths it doesn't use.
        "/tmdb/account/1",
        "/tmdb/movie/upcoming",
        "/tmdb/movie/7/videos",
        "/tmdb/movie/abc",
        // Parameters it doesn't send, or sends only with one value, or only once.
        "/tmdb/movie/7?append_to_response=images,videos,reviews",
        "/tmdb/movie/7?append_to_response=keywords,credits&x=nonce",
        "/tmdb/movie/popular?page=1&page=2",
        "/tmdb/discover/movie?sort_by=revenue.desc",
        "/tmdb/person/3/movie_credits?language=fr",
        // A key of its own.
        "/tmdb/movie/7?append_to_response=keywords,credits&api_key=someone-elses",
    ] {
        assert_eq!(
            status(&url, refused).await,
            reqwest::StatusCode::FORBIDDEN,
            "{refused}"
        );
    }
    assert_eq!(seen.requests.load(Ordering::SeqCst), 0);
    // The app's own shape still passes.
    let allowed = "/tmdb/movie/7?append_to_response=keywords,credits";
    assert!(status(&url, allowed).await.is_success());
}

#[tokio::test]
async fn a_refused_request_is_an_error_not_a_missing_movie() {
    let (url, _) = start(None).await;
    // A client that sends its own key: the server refuses it, as it would any
    // request shape it doesn't know.
    let mismatched =
        TmdbClient::with_base_url(http(), "client-key".to_string(), format!("{url}/tmdb"));

    let err = mismatched
        .movie_details(7)
        .await
        .expect_err("refused by the server");

    assert!(!is_not_found(&err), "{err:#}");
    assert!(format!("{err:#}").contains("403"), "{err:#}");
}

#[tokio::test]
async fn requests_that_print_alike_are_cached_apart() {
    let dir = temp_dir("collision");
    let (url, seen) = start(Some(&dir)).await;

    // One year value containing "&query=Alien", then the two parameters it looks
    // like once sorted. The second must not get the first's cached answer.
    let one_value = http()
        .get(format!(
            "{url}/tmdb/search/movie?primary_release_year=1979%26query%3DAlien"
        ))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    let two_params = http()
        .get(format!(
            "{url}/tmdb/search/movie?primary_release_year=1979&query=Alien"
        ))
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    let _ = std::fs::remove_dir_all(&dir);

    // The stand-in names its result after the query, and the first request had none.
    assert!(one_value.contains(r#""title":"""#), "{one_value}");
    assert!(two_params.contains(r#""title":"Alien""#), "{two_params}");
    assert_eq!(seen.requests.load(Ordering::SeqCst), 2);
}
