//! Thin client for The Movie Database (TMDB) API.

pub mod models;

pub use models::Movie;

const BASE_URL: &str = "https://api.themoviedb.org/3";

pub struct TmdbClient {
    http: reqwest::Client,
    api_key: String,
}

impl TmdbClient {
    pub fn new(api_key: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
        }
    }

    /// Movies releasing soon, from TMDB's `/movie/upcoming` endpoint.
    pub async fn upcoming(&self) -> anyhow::Result<Vec<Movie>> {
        let url = format!("{BASE_URL}/movie/upcoming");
        let page: models::Page<Movie> = self
            .http
            .get(url)
            .query(&[("api_key", self.api_key.as_str())])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(page.results)
    }
}
