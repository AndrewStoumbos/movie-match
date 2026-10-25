//! Thin client for The Movie Database (TMDB) API.

pub mod models;

use anyhow::Context;

pub use models::Movie;

const BASE_URL: &str = "https://api.themoviedb.org/3";

pub struct TmdbClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl TmdbClient {
    pub fn new(api_key: String) -> Self {
        Self::with_base_url(api_key, BASE_URL.to_string())
    }

    /// A client that sends requests to `base_url` instead of the public TMDB API.
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            base_url,
        }
    }

    /// Movies releasing soon, from TMDB's `/movie/upcoming` endpoint.
    pub async fn upcoming(&self) -> anyhow::Result<Vec<Movie>> {
        let url = format!("{}/movie/upcoming", self.base_url);
        // reqwest errors carry the request URL, whose query string holds the
        // API key, so every error is stripped of its URL before it can surface.
        let page: models::Page<Movie> = self
            .http
            .get(url)
            .query(&[("api_key", self.api_key.as_str())])
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(reqwest::Error::without_url)
            .context("TMDB request for upcoming movies failed")?
            .json()
            .await
            .map_err(reqwest::Error::without_url)
            .context("could not parse TMDB upcoming movies response")?;
        Ok(page.results)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    const FAKE_KEY: &str = "fake-key-0123456789abcdef";

    /// Serves one canned HTTP response on localhost and returns its base URL.
    fn serve_once(response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    async fn upcoming_error(base_url: String) -> String {
        let client = TmdbClient::with_base_url(FAKE_KEY.to_string(), base_url);
        let err = client.upcoming().await.unwrap_err();
        // `{:?}` on anyhow::Error prints the whole cause chain.
        format!("{err:?}")
    }

    #[tokio::test]
    async fn http_status_error_does_not_leak_api_key() {
        let base = serve_once(
            "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        );
        let msg = upcoming_error(base).await;
        assert!(msg.contains("401"), "unexpected error: {msg}");
        assert!(!msg.contains(FAKE_KEY), "API key leaked: {msg}");
    }

    #[tokio::test]
    async fn invalid_json_error_does_not_leak_api_key() {
        let base = serve_once(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 8\r\nconnection: close\r\n\r\nnot json",
        );
        let msg = upcoming_error(base).await;
        assert!(msg.contains("parse"), "unexpected error: {msg}");
        assert!(!msg.contains(FAKE_KEY), "API key leaked: {msg}");
    }

    #[tokio::test]
    async fn connection_error_does_not_leak_api_key() {
        // Bind then drop a listener so the port is very likely closed.
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let msg = upcoming_error(format!("http://{addr}")).await;
        assert!(!msg.contains(FAKE_KEY), "API key leaked: {msg}");
    }
}
