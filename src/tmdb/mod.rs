//! Thin client for The Movie Database (TMDB) API.

pub mod models;

pub use models::{CreditedFilm, Movie, MovieDetails, Page, PersonCredits};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

const BASE_URL: &str = "https://api.themoviedb.org/3";

/// A country as TMDB names it: an ISO 3166-1 two-letter code such as `US` or `GB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Region([u8; 2]);

impl Region {
    /// The region for a two-letter code in either case, ignoring surrounding
    /// whitespace; `None` for anything else.
    pub fn parse(code: &str) -> Option<Self> {
        match code.trim().as_bytes() {
            &[a, b] if a.is_ascii_alphabetic() && b.is_ascii_alphabetic() => {
                Some(Self([a.to_ascii_uppercase(), b.to_ascii_uppercase()]))
            }
            _ => None,
        }
    }

    /// The country in a locale name such as `en-US`, `zh-Hans-CN` or `en_GB.UTF-8`;
    /// `None` when it names only a language, like `en`.
    pub fn from_locale(locale: &str) -> Option<Self> {
        let name = locale.split('.').next().unwrap_or_default();
        let mut parts = name.split(['-', '_']);
        parts.next(); // The language.
        parts.find(|part| part.len() == 2).and_then(Self::parse)
    }

    pub fn as_str(&self) -> &str {
        // Both bytes are ASCII letters, so this is always valid UTF-8.
        std::str::from_utf8(&self.0).unwrap_or_default()
    }
}

impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What to ask `/discover/movie` for: see [`TmdbClient::discover`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Discover<'a> {
    /// Earliest primary release date (inclusive, `YYYY-MM-DD`); `None` leaves it open.
    pub from: Option<&'a str>,
    /// Latest primary release date (inclusive, `YYYY-MM-DD`); `None` leaves it open.
    pub to: Option<&'a str>,
    /// Fewest audience votes; 0 for no minimum.
    pub min_votes: u32,
    /// Only movies with a theatrical release (limited or wide) listed for this
    /// country, dated between `from` and `to` too.
    pub theatrical_in: Option<Region>,
}

/// Whether `err` comes from TMDB answering 404 Not Found, meaning it has no such
/// resource (say a movie it has since removed). Other failures, such as timeouts,
/// rate limits, server errors or undecodable bodies, are not.
pub fn is_not_found(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|e| e.downcast_ref::<reqwest::Error>())
        .any(|e| e.status() == Some(reqwest::StatusCode::NOT_FOUND))
}

pub struct TmdbClient {
    http: reqwest::Client,
    /// `None` when talking to a Movie Match server, which adds its own key.
    api_key: Option<String>,
    base_url: String,
}

/// Where a Movie Match server serves TMDB requests, under its address.
pub const SERVER_TMDB_PATH: &str = "/tmdb";

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
            api_key: Some(api_key),
            base_url,
        }
    }

    /// A client that asks the Movie Match server at `server_url` (e.g.
    /// `http://127.0.0.1:8765`) instead of TMDB. It sends no API key: the server
    /// holds one and passes TMDB's answers through.
    pub fn via_server(server_url: &str) -> Self {
        Self::via_server_using(reqwest::Client::new(), server_url)
    }

    /// [`Self::via_server`] through `http`, e.g. one that ignores proxies in tests
    /// (see [`Self::with_base_url`]).
    pub fn via_server_using(http: reqwest::Client, server_url: &str) -> Self {
        Self {
            http,
            api_key: None,
            base_url: format!("{}{SERVER_TMDB_PATH}", server_url.trim_end_matches('/')),
        }
    }

    /// One page of currently popular movies, from `/movie/popular`.
    pub async fn popular(&self, page: u32) -> Result<Page<Movie>> {
        self.get("/movie/popular", &[("page", page.to_string())])
            .await
    }

    /// One page of the movies `query` describes, most popular first, from
    /// `/discover/movie`.
    pub async fn discover(&self, query: Discover<'_>, page: u32) -> Result<Page<Movie>> {
        let mut params = vec![
            ("sort_by", "popularity.desc".to_string()),
            ("include_adult", "false".to_string()),
            ("include_video", "false".to_string()),
            ("page", page.to_string()),
        ];
        if query.min_votes > 0 {
            params.push(("vote_count.gte", query.min_votes.to_string()));
        }
        if let Some(from) = query.from {
            params.push(("primary_release_date.gte", from.to_string()));
        }
        if let Some(to) = query.to {
            params.push(("primary_release_date.lte", to.to_string()));
        }
        if let Some(region) = query.theatrical_in {
            // Release types 2 and 3: theatrical (limited) or theatrical. TMDB applies
            // them only with `release_date` bounds, which match that country's
            // release dates rather than the first one anywhere.
            params.push(("region", region.to_string()));
            params.push(("with_release_type", "2|3".to_string()));
            if let Some(from) = query.from {
                params.push(("release_date.gte", from.to_string()));
            }
            if let Some(to) = query.to {
                params.push(("release_date.lte", to.to_string()));
            }
        }
        self.get("/discover/movie", &params).await
    }

    /// TMDB's best match for `title`, optionally narrowed to a release year.
    pub async fn search_movie(&self, title: &str, year: Option<u16>) -> Result<Option<Movie>> {
        Ok(self.search_movies(title, year).await?.into_iter().next())
    }

    /// The first page of TMDB's matches for `title`, best first, optionally narrowed
    /// to a release year.
    pub async fn search_movies(&self, title: &str, year: Option<u16>) -> Result<Vec<Movie>> {
        let mut params = vec![("query", title.to_string())];
        if let Some(year) = year {
            params.push(("primary_release_year", year.to_string()));
        }
        let page: Page<Movie> = self.get("/search/movie", &params).await?;
        Ok(page.results)
    }

    /// Genres, keywords and credits for one movie, fetched in a single request.
    pub async fn movie_details(&self, id: u64) -> Result<MovieDetails> {
        let params = [("append_to_response", "keywords,credits".to_string())];
        self.get(&format!("/movie/{id}"), &params).await
    }

    /// Every movie one person acted in or worked on.
    pub async fn person_movie_credits(&self, id: u64) -> Result<PersonCredits> {
        self.get(&format!("/person/{id}/movie_credits"), &[]).await
    }

    /// GETs `path` and decodes the JSON body.
    ///
    /// The API key is sent as a query parameter and reqwest errors embed the request
    /// URL, so every error is stripped of its URL before it can reach a message or log.
    async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, String)]) -> Result<T> {
        let mut request = self.http.get(format!("{}{path}", self.base_url));
        if let Some(key) = &self.api_key {
            request = request.query(&[("api_key", key)]);
        }
        let response = request
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
