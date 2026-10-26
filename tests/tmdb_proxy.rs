//! Test clients must reach the local stand-in server even when the environment
//! configures a proxy.
//!
//! This file holds a single test because it changes process-wide environment
//! variables; cargo runs each integration test file as its own process.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use movie_match::tmdb::TmdbClient;

#[tokio::test]
async fn no_proxy_client_ignores_proxy_env_vars() {
    // A proxy on a loopback port nothing listens on: any request routed through it
    // fails to connect instead of reaching the stand-in server.
    let dead_proxy = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("reserve loopback port");
    for var in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::set_var(var, format!("http://{dead_proxy}"));
    }
    for var in ["NO_PROXY", "no_proxy"] {
        std::env::remove_var(var);
    }

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
        let body = r#"{"page":1,"results":[],"total_pages":1,"total_results":0}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });

    let http = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build HTTP client");
    let client = TmdbClient::with_base_url(http, "test-key".to_string(), format!("http://{addr}"));

    let page = client
        .upcoming(1)
        .await
        .expect("request bypasses the proxy and reaches the stand-in server");
    assert!(page.results.is_empty());
}
