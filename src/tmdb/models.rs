use serde::Deserialize;

/// One page of results from a paginated TMDB endpoint.
#[derive(Debug, Deserialize)]
pub struct Page<T> {
    pub page: u32,
    pub results: Vec<T>,
    pub total_pages: u32,
}

/// The subset of TMDB movie fields we use.
#[derive(Debug, Clone, Deserialize)]
pub struct Movie {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub overview: String,
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub genre_ids: Vec<u32>,
    #[serde(default)]
    pub vote_average: f32,
}
