//! TMDB client errors must never reveal the API key.
//!
//! The key travels in the request URL, and reqwest errors embed that URL, so each test
//! makes a request fail in a different way against a local stand-in server and checks
//! every rendering of the resulting error.

mod common;

use std::net::TcpListener;

use common::tmdb::{local_client, serve_once, API_KEY as SECRET_KEY};
use movie_match::tmdb::is_not_found;

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

    let err = client.upcoming(1).await.expect_err("401 is an error");

    assert!(format!("{err:#}").contains("/movie/upcoming"));
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
