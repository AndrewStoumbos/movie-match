//! The shared test client must reach the local stand-in server even when the
//! environment configures a proxy.
//!
//! This guards `common::tmdb::local_client`, which the other TMDB client tests rely on.
//! This file holds a single test because it changes process-wide environment
//! variables; cargo runs each integration test file as its own process.

mod common;

use std::net::TcpListener;

use common::tmdb::{local_client, serve_once};

#[tokio::test]
async fn local_client_ignores_proxy_env_vars() {
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

    let body = r#"{"page":1,"results":[],"total_pages":1,"total_results":0}"#;
    let base = serve_once(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    let client = local_client(base);

    let page = client
        .popular(1)
        .await
        .expect("request bypasses the proxy and reaches the stand-in server");
    assert!(page.results.is_empty());
}
