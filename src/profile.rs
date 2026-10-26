//! The user's rated movies and the taste profile built from them.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::features::{self, FieldWeights};
use crate::tfidf::{Idf, Vector};
use crate::tmdb::MovieDetails;

/// Rating assumed for entries without one: a plain list of titles means favourites.
pub const UNRATED: f32 = 10.0;
/// Ratings at or above this count as liked.
pub const LIKED_FROM: f32 = 7.0;
/// Ratings at or below this count as disliked. Ratings in between are neutral.
pub const DISLIKED_UP_TO: f32 = 4.0;

#[derive(Debug, Deserialize)]
struct RatingsFile {
    movies: Vec<RatedMovie>,
}

/// A movie the user has rated, from a ratings file or an imported account.
///
/// Identified by `tmdb_id` when known; otherwise by `title` (and `year`) through
/// a TMDB search.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RatedMovie {
    pub tmdb_id: Option<u64>,
    pub title: Option<String>,
    pub year: Option<u16>,
    /// 0.5 to 10. Missing means a favourite (see [`UNRATED`]).
    pub rating: Option<f32>,
}

impl RatedMovie {
    pub fn rating(&self) -> f32 {
        self.rating.unwrap_or(UNRATED)
    }

    /// A short name for messages: the title, or else the TMDB id.
    pub fn label(&self) -> String {
        match (&self.title, self.tmdb_id) {
            (Some(title), _) => title.clone(),
            (None, Some(id)) => format!("TMDB movie {id}"),
            (None, None) => "untitled entry".to_string(),
        }
    }
}

pub fn load_rated_movies(path: &Path) -> Result<Vec<RatedMovie>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    parse_rated_movies(&text).with_context(|| format!("could not parse {}", path.display()))
}

/// Parses and validates the TOML ratings format (see `data/liked_movies.example.toml`).
pub fn parse_rated_movies(text: &str) -> Result<Vec<RatedMovie>> {
    let file: RatingsFile = toml::from_str(text)?;
    for (i, movie) in file.movies.iter().enumerate() {
        let entry = i + 1;
        let no_title = movie.title.as_deref().is_none_or(|t| t.trim().is_empty());
        if movie.tmdb_id.is_none() && no_title {
            bail!("movie #{entry} needs a title or a tmdb_id");
        }
        if let Some(rating) = movie.rating {
            if !(0.5..=10.0).contains(&rating) {
                bail!(
                    "movie #{entry} ({}) is rated {rating}; ratings go from 0.5 to 10",
                    movie.label()
                );
            }
        }
    }
    Ok(file.movies)
}

/// A rated movie with its full TMDB details.
#[derive(Debug, Clone)]
pub struct RatedDetails {
    pub movie: MovieDetails,
    pub rating: f32,
}

/// One liked or disliked movie, ready to compare against candidates.
#[derive(Debug, Clone)]
pub struct RatedVector {
    pub title: String,
    pub rating: f32,
    /// How strongly resemblance to this movie moves a score, 0.0 to 1.0.
    pub strength: f32,
    pub vector: Vector,
}

/// Each liked and disliked movie as its own TF-IDF vector, kept separate rather
/// than averaged so that eclectic tastes (say, horror and romantic comedy) don't
/// blur together.
#[derive(Debug, Clone)]
pub struct TasteProfile {
    idf: Idf,
    weights: FieldWeights,
    liked: Vec<RatedVector>,
    disliked: Vec<RatedVector>,
    rated_ids: HashSet<u64>,
}

impl TasteProfile {
    /// Sorts `rated` into liked and disliked movies. Neutral ratings only mark the
    /// movie as already seen.
    pub fn new(rated: &[RatedDetails], idf: Idf, weights: FieldWeights) -> Self {
        let mut liked = Vec::new();
        let mut disliked = Vec::new();
        for r in rated {
            let (list, strength) = if r.rating >= LIKED_FROM {
                // 7/10 pulls at 0.7, 10/10 at full strength.
                (&mut liked, r.rating / 10.0)
            } else if r.rating <= DISLIKED_UP_TO {
                // 4/10 pushes at 0.25, 1/10 or lower at full strength.
                (&mut disliked, ((5.0 - r.rating) / 4.0).min(1.0))
            } else {
                continue;
            };
            list.push(RatedVector {
                title: r.movie.title.clone(),
                rating: r.rating,
                strength,
                vector: Vector::new(&features::extract(&r.movie, &weights), &idf),
            });
        }
        Self {
            idf,
            weights,
            liked,
            disliked,
            rated_ids: rated.iter().map(|r| r.movie.id).collect(),
        }
    }

    pub fn liked(&self) -> &[RatedVector] {
        &self.liked
    }

    pub fn disliked(&self) -> &[RatedVector] {
        &self.disliked
    }

    pub fn weights(&self) -> &FieldWeights {
        &self.weights
    }

    /// Whether the user has rated (and so has presumably seen) this TMDB movie.
    pub fn has_rated(&self, tmdb_id: u64) -> bool {
        self.rated_ids.contains(&tmdb_id)
    }

    /// `movie` as a TF-IDF vector comparable with the rated movies.
    pub fn vectorize(&self, movie: &MovieDetails) -> Vector {
        Vector::new(&features::extract(movie, &self.weights), &self.idf)
    }
}
