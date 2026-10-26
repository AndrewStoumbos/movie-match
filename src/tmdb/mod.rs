//! Thin client for The Movie Database (TMDB) API.

pub mod models;

pub use models::{Movie, MovieDetails, Page};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

const BASE_URL: &str = "https://api.themoviedb.org/3";

pub struct TmdbClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl TmdbClient {
    pub fn new(api_key: String) -> Self {
        Self::with_base_url(reqwest::Client::new(), api_key, BASE_URL.to_string())
    }

    /// A client that sends requests through `http` to `base_url` instead of the real
    /// TMDB API, e.g. a local stand-in server in tests.
    ///
    /// The caller supplies `http` so tests can build it with
    /// [`reqwest::ClientBuilder::no_proxy`]; otherwise proxy environment variables
    /// could route loopback requests to a proxy instead of the stand-in server.
    pub fn with_base_url(http: reqwest::Client, api_key: String, base_url: String) -> Self {
        Self {
            http,
            api_key,
            base_url,
        }
    }

    /// One page of movies releasing soon, from `/movie/upcoming`.
    pub async fn upcoming(&self, page: u32) -> Result<Page<Movie>> {
        self.get("/movie/upcoming", &[("page", page.to_string())])
            .await
    }

    /// One page of currently popular movies, from `/movie/popular`.
    pub async fn popular(&self, page: u32) -> Result<Page<Movie>> {
        self.get("/movie/popular", &[("page", page.to_string())])
            .await
    }

    /// TMDB's best match for `title`, optionally narrowed to a release year.
    pub async fn search_movie(&self, title: &str, year: Option<u16>) -> Result<Option<Movie>> {
        let mut params = vec![("query", title.to_string())];
        if let Some(year) = year {
            params.push(("primary_release_year", year.to_string()));
        }
        let page: Page<Movie> = self.get("/search/movie", &params).await?;
        Ok(page.results.into_iter().next())
    }

    /// Genres, keywords and credits for one movie, fetched in a single request.
    pub async fn movie_details(&self, id: u64) -> Result<MovieDetails> {
        let params = [("append_to_response", "keywords,credits".to_string())];
        self.get(&format!("/movie/{id}"), &params).await
    }

    /// GETs `path` and decodes the JSON body.
    ///
    /// The API key is sent as a query parameter and reqwest errors embed the request
    /// URL, so every error is stripped of its URL before it can reach a message or log.
    async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, String)]) -> Result<T> {
        let response = self
            .http
            .get(format!("{}{path}", self.base_url))
            .query(&[("api_key", self.api_key.as_str())])
            .query(params)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("TMDB request to {path} failed"))?;
        response
            .json()
            .await
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("could not decode TMDB response from {path}"))
    }
}
