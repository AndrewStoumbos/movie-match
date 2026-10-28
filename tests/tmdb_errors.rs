//! TMDB client errors must never reveal the API key.
//!
//! The key travels in the request URL, and reqwest errors embed that URL, so each test
//! makes a request fail in a different way against a local stand-in server and checks
//! every rendering of the resulting error.

mod common;

use std::net::TcpListener;

use common::tmdb::{local_client, serve_once, serve_once_recording, API_KEY as SECRET_KEY};
use movie_match::tmdb::{is_not_found, TmdbClient};

fn assert_key_not_leaked(err: &anyhow::Error) {
    for rendered in [
        format!("{err}"),
        format!("{err:#}"),
        format!("{err:?}"),
        format!("{err:#?}"),
    ] {
        assert!(
            !rendered.contains(SECRET_KEY),
            "error leaks the API key: {rendered}"
        );
        assert!(
            !rendered.contains("api_key"),
            "error leaks the request URL: {rendered}"
        );
    }
}

#[tokio::test]
async fn http_error_status_does_not_leak_key() {
    let base =
        serve_once("HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let client = local_client(base);

    let err = client.popular(1).await.expect_err("401 is an error");

    assert!(format!("{err:#}").contains("/movie/popular"));
    assert!(format!("{err:#}").contains("401"));
    assert_key_not_leaked(&err);
}

#[tokio::test]
async fn undecodable_body_does_not_leak_key() {
    let base = serve_once(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot json",
    );
    let client = local_client(base);

    let err = client
        .movie_details(42)
        .await
        .expect_err("invalid JSON is an error");

    assert!(format!("{err:#}").contains("could not decode"));
    assert_key_not_leaked(&err);
}

#[tokio::test]
async fn connection_failure_does_not_leak_key() {
    // Bind and immediately drop a listener to get a loopback port nothing listens on.
    let addr = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("reserve loopback port");
    let client = local_client(format!("http://{addr}"));

    let err = client
        .search_movie("Heat", Some(1995))
        .await
        .expect_err("refused connection is an error");

    assert!(format!("{err:#}").contains("/search/movie"));
    assert_key_not_leaked(&err);
}

#[tokio::test]
async fn only_404_counts_as_not_found() {
    let not_found = local_client(serve_once(
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ))
    .movie_details(42)
    .await
    .expect_err("404 is an error")
    .context("outer context");
    assert!(is_not_found(&not_found), "{not_found:#}");

    for response in [
        "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot json",
    ] {
        let err = local_client(serve_once(response))
            .movie_details(42)
            .await
            .expect_err("request fails");
        assert!(!is_not_found(&err), "{err:#}");
    }
}

#[tokio::test]
async fn a_server_client_sends_no_key_and_asks_under_tmdb() {
    let body = r#"{"page":1,"results":[],"total_pages":1,"total_results":0}"#;
    let (base, request) = serve_once_recording(format!(
        "HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: {}
Connection: close

{body}",
        body.len()
    ));
    let http = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build HTTP client");
    // A trailing slash on the address is fine.
    let client = TmdbClient::via_server_using(http, &format!("{base}/"));

    client.popular(1).await.expect("an empty page");

    let request = request
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("server saw the request");
    let line = request.lines().next().unwrap_or_default();
    assert!(line.starts_with("GET /tmdb/movie/popular?"), "{line}");
    assert!(!line.contains("api_key"), "{line}");
}
