//! The Movie Match server: holds the TMDB API key and answers the app's TMDB
//! requests (see `TmdbClient::via_server`), so users never need a key of their own.
//!
//! It forwards only the requests the app makes, adds its key, passes TMDB's answer
//! and status through, and caches successful answers for everyone: the catalog and
//! track records are the same for every user, so one fetch serves them all.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use movie_match::cache::DiskCache;
use movie_match::catalog::CATALOG_MAX_AGE;
use movie_match::fetch::{DETAILS_MAX_AGE, FILMOGRAPHY_MAX_AGE};
use movie_match::tmdb::SERVER_TMDB_PATH;
use serde::{Deserialize, Serialize};

/// The real TMDB API.
pub const TMDB_URL: &str = "https://api.themoviedb.org/3";

/// How long the server waits for TMDB before answering 502.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Server {
    http: reqwest::Client,
    tmdb_url: String,
    api_key: String,
    cache: Option<DiskCache>,
}

impl Server {
    /// A server that asks `tmdb_url` (normally [`TMDB_URL`]) with `api_key`, caching
    /// answers in `cache` if given.
    pub fn new(
        api_key: String,
        tmdb_url: String,
        cache: Option<DiskCache>,
    ) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(UPSTREAM_TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            tmdb_url,
            api_key,
            cache,
        })
    }
}

pub fn router(server: Arc<Server>) -> Router {
    Router::new()
        .route(&format!("{SERVER_TMDB_PATH}/{{*path}}"), get(tmdb))
        .route("/health", get(|| async { "ok" }))
        .with_state(server)
}

/// A query parameter the app sends: its name, and its value when that's always
/// the same.
type Param = (&'static str, Option<&'static str>);

const POPULAR: &[Param] = &[("page", None)];
const DISCOVER: &[Param] = &[
    ("sort_by", Some("popularity.desc")),
    ("include_adult", Some("false")),
    ("include_video", Some("false")),
    ("page", None),
    ("vote_count.gte", None),
    ("primary_release_date.gte", None),
    ("primary_release_date.lte", None),
    ("region", None),
    ("with_release_type", Some("2|3")),
    ("release_date.gte", None),
    ("release_date.lte", None),
];
const SEARCH: &[Param] = &[("query", None), ("primary_release_year", None)];
const DETAILS: &[Param] = &[("append_to_response", Some("keywords,credits"))];

/// How long TMDB's answer to `path` with `params` is reused, or `None` for a request
/// the app never makes: another path, a parameter `TmdbClient` doesn't send (an API
/// key included), a repeated one, or a fixed one with another value. Refusing those
/// keeps the server from serving the rest of TMDB's API on its key, and from caching
/// endless variants of one request.
pub fn allowed(path: &str, params: &[(String, String)]) -> Option<Duration> {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let (max_age, known) = match parts.as_slice() {
        ["movie", "popular"] => (CATALOG_MAX_AGE, POPULAR),
        ["discover", "movie"] => (CATALOG_MAX_AGE, DISCOVER),
        ["search", "movie"] => (CATALOG_MAX_AGE, SEARCH),
        ["movie", id] if number(id) => (DETAILS_MAX_AGE, DETAILS),
        ["person", id, "movie_credits"] if number(id) => (FILMOGRAPHY_MAX_AGE, &[][..]),
        _ => return None,
    };
    let all_known = params.iter().enumerate().all(|(i, (name, value))| {
        let once = !params[..i].iter().any(|(earlier, _)| earlier == name);
        let fits = known
            .iter()
            .any(|(k, fixed)| k == name && fixed.is_none_or(|f| f == value));
        once && fits
    });
    all_known.then_some(max_age)
}

/// A cached answer, with the exact request it answers, so a hash collision between
/// two requests is a miss rather than one user's answer served for another's.
#[derive(Serialize, Deserialize)]
struct Cached {
    path: String,
    params: Vec<(String, String)>,
    body: String,
}

async fn tmdb(
    State(server): State<Arc<Server>>,
    Path(path): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    let path = format!("/{path}");
    let mut params = query;
    // 403, not 404: the app reads a 404 as "TMDB has no such movie", which would
    // hide a client and server that disagree about what's allowed.
    let Some(max_age) = allowed(&path, &params) else {
        return (StatusCode::FORBIDDEN, "not a request Movie Match makes").into_response();
    };
    // In a fixed order so equal requests share one cache entry.
    params.sort();
    // Hashed from the JSON form, which escapes every name and value, so different
    // requests can't run together into the same text.
    let request = serde_json::to_string(&(&path, &params)).unwrap_or_default();
    let key = format!("tmdb-v2/{:016x}", fnv1a(&request));
    if let Some(cached) = server
        .cache
        .as_ref()
        .and_then(|c| c.get::<Cached>(&key, max_age))
        .filter(|c| c.path == path && c.params == params)
    {
        return json(cached.body);
    }

    let answer = server
        .http
        .get(format!("{}{path}", server.tmdb_url))
        .query(&[("api_key", server.api_key.as_str())])
        .query(&params)
        .send()
        .await
        .map_err(reqwest::Error::without_url);
    let answer = match answer {
        Ok(answer) => answer,
        Err(err) => {
            eprintln!("warning: TMDB request to {path} failed: {err}");
            return (StatusCode::BAD_GATEWAY, "TMDB request failed").into_response();
        }
    };
    let status = answer.status().as_u16();
    let body = match answer.text().await {
        Ok(body) => body,
        Err(err) => {
            eprintln!(
                "warning: TMDB answer from {path} failed: {}",
                err.without_url()
            );
            return (StatusCode::BAD_GATEWAY, "TMDB answer failed").into_response();
        }
    };
    if status != 200 {
        // Passed through unchanged, so the app sees TMDB's 404s and rate limits.
        let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
        return (status, body).into_response();
    }
    if let Some(cache) = &server.cache {
        cache.put_or_warn(
            &key,
            &Cached {
                path,
                params,
                body: body.clone(),
            },
        );
    }
    json(body)
}

fn json(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// 64-bit FNV-1a: a stable hash for cache file names (std's hasher may change
/// between Rust releases).
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
