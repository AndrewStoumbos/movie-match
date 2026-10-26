//! TMDB client errors must never reveal the API key.
//!
//! The key travels in the request URL, and reqwest errors embed that URL, so each test
//! makes a request fail in a different way against a local stand-in server and checks
//! every rendering of the resulting error.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use movie_match::tmdb::TmdbClient;

const SECRET_KEY: &str = "not-a-real-key-5ec7e7";

/// Serves exactly one HTTP request on a loopback port with `response`, and returns
/// the base URL to send it to.
fn serve_once(response: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let addr = listener.local_addr().expect("listener address");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept connection");
        let mut request = Vec::new();
        let mut buf = [0; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => request.extend_from_slice(&buf[..n]),
            }
        }
        let _ = stream.write_all(response.as_bytes());
    });
    format!("http://{addr}")
}

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
    let client = TmdbClient::with_base_url(SECRET_KEY.to_string(), base);

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
    let client = TmdbClient::with_base_url(SECRET_KEY.to_string(), base);

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
    let client = TmdbClient::with_base_url(SECRET_KEY.to_string(), format!("http://{addr}"));

    let err = client
        .search_movie("Heat", Some(1995))
        .await
        .expect_err("refused connection is an error");

    assert!(format!("{err:#}").contains("/search/movie"));
    assert_key_not_leaked(&err);
}
