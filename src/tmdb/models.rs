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
    /// Plot summary. TMDB sends an empty string or null when there isn't one.
    #[serde(default)]
    pub overview: Option<String>,
    /// Path of the poster image, e.g. `/abc123.jpg`; see [`MovieDetails::poster_url`].
    #[serde(default)]
    pub poster_path: Option<String>,
    /// TMDB users' average rating, 0 to 10 (0 when nobody has voted).
    #[serde(default)]
    pub vote_average: Option<f32>,
    #[serde(default)]
    pub vote_count: Option<u32>,
}

/// Base address of TMDB's image server.
const IMAGE_BASE_URL: &str = "https://image.tmdb.org/t/p";

impl MovieDetails {
    /// TMDB users' average rating (0 to 10), if anyone has voted.
    pub fn audience_rating(&self) -> Option<f32> {
        match (self.vote_average, self.vote_count) {
            (Some(average), Some(count)) if count > 0 => Some(average),
            _ => None,
        }
    }

    /// Full URL of the poster at a TMDB size such as `"w185"`, if the movie has one.
    pub fn poster_url(&self, size: &str) -> Option<String> {
        self.poster_path
            .as_deref()
            .filter(|path| !path.is_empty())
            .map(|path| format!("{IMAGE_BASE_URL}/{size}{path}"))
    }

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

/// A person's movies, from `/person/{id}/movie_credits`, acted in or crewed.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PersonCredits {
    #[serde(default)]
    pub cast: Vec<CreditedFilm>,
    #[serde(default)]
    pub crew: Vec<CreditedFilm>,
}

impl PersonCredits {
    /// Each distinct film once, keeping only those with a release date and audience
    /// votes: all a track record needs, and far smaller to cache.
    pub fn films(self) -> Vec<CreditedFilm> {
        let mut seen = std::collections::HashSet::new();
        self.cast
            .into_iter()
            .chain(self.crew)
            .filter(|film| film.released().is_some() && film.votes() > 0)
            .filter(|film| seen.insert(film.id))
            .collect()
    }
}

/// One film in a person's credits, with only the fields a track record uses.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct CreditedFilm {
    pub id: u64,
    /// `YYYY-MM-DD`; TMDB sends an empty string or null when unknown.
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub vote_average: Option<f32>,
    #[serde(default)]
    pub vote_count: Option<u32>,
}

impl CreditedFilm {
    /// The release date, if known.
    pub fn released(&self) -> Option<&str> {
        self.release_date.as_deref().filter(|d| !d.is_empty())
    }

    pub fn votes(&self) -> u32 {
        self.vote_count.unwrap_or(0)
    }
}
