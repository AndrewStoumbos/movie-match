//! A local stand-in for the TMDB API, so client tests never need the network.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use movie_match::tmdb::TmdbClient;

/// The API key every local client sends. Tests check that errors never reveal it.
pub const API_KEY: &str = "not-a-real-key-5ec7e7";

/// Serves exactly one HTTP request on a loopback port with `response`, and returns
/// the base URL to send it to.
pub fn serve_once(response: impl Into<String>) -> String {
    serve_once_recording(response).0
}

/// Like [`serve_once`], but also hands back the raw request head (request line and
/// headers) once it has been received.
pub fn serve_once_recording(response: impl Into<String>) -> (String, Receiver<String>) {
    let response = response.into();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let addr = listener.local_addr().expect("listener address");
    let (sender, receiver) = mpsc::channel();
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
        let _ = sender.send(String::from_utf8_lossy(&request).into_owned());
    });
    (format!("http://{addr}"), receiver)
}

/// A client for the stand-in server at `base` that sends [`API_KEY`]. It ignores proxy
/// environment variables, so loopback requests are never sent through a proxy to
/// another host.
pub fn local_client(base: String) -> TmdbClient {
    let http = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build HTTP client");
    TmdbClient::with_base_url(http, API_KEY.to_string(), base)
}
