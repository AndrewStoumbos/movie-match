use serde::{Deserialize, Serialize};

/// One page of results from a paginated TMDB endpoint.
#[derive(Debug, Deserialize)]
pub struct Page<T> {
    pub page: u32,
    pub results: Vec<T>,
    pub total_pages: u32,
}

/// A movie as it appears in TMDB list and search results.
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

/// Full details for one movie, from `/movie/{id}?append_to_response=keywords,credits`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MovieDetails {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub genres: Vec<Genre>,
    #[serde(default)]
    pub keywords: Keywords,
    #[serde(default)]
    pub credits: Credits,
}

impl MovieDetails {
    /// Crew members credited with `job` (e.g. "Director"), in credit order.
    pub fn crew_with_job<'a>(&'a self, job: &'a str) -> impl Iterator<Item = &'a CrewMember> {
        self.credits.crew.iter().filter(move |c| c.job == job)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Genre {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Keywords {
    #[serde(default)]
    pub keywords: Vec<Keyword>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Keyword {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Credits {
    #[serde(default)]
    pub cast: Vec<CastMember>,
    #[serde(default)]
    pub crew: Vec<CrewMember>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CastMember {
    pub id: u64,
    pub name: String,
    /// Billing position: 0 is the top-billed actor.
    #[serde(default)]
    pub order: u32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CrewMember {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub job: String,
}
