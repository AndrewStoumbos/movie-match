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

/// One rated movie, ready to compare against candidates.
#[derive(Debug, Clone)]
pub struct RatedVector {
    pub title: String,
    pub rating: f32,
    pub vector: Vector,
}

impl RatedVector {
    pub fn is_liked(&self) -> bool {
        self.rating >= LIKED_FROM
    }

    pub fn is_disliked(&self) -> bool {
        self.rating <= DISLIKED_UP_TO
    }
}

/// Every rated movie as its own TF-IDF vector, kept separate rather than averaged
/// so that eclectic tastes (say, horror and romantic comedy) don't blur together.
#[derive(Debug, Clone)]
pub struct TasteProfile {
    idf: Idf,
    weights: FieldWeights,
    rated: Vec<RatedVector>,
    mean_rating: f32,
    low_rating_cutoff: f32,
    rated_ids: HashSet<u64>,
}

/// Share of a user's ratings that count as their low ratings (see
/// [`TasteProfile::is_low_rating`]).
pub const LOW_RATING_SHARE: f32 = 0.4;

impl TasteProfile {
    pub fn new(rated: &[RatedDetails], idf: Idf, weights: FieldWeights) -> Self {
        let vectors = rated
            .iter()
            .map(|r| RatedVector {
                title: r.movie.title.clone(),
                rating: r.rating,
                vector: Vector::new(&features::extract(&r.movie, &weights), &idf),
            })
            .collect();
        let mean_rating = if rated.is_empty() {
            0.0
        } else {
            rated.iter().map(|r| r.rating).sum::<f32>() / rated.len() as f32
        };
        Self {
            idf,
            weights,
            rated: vectors,
            mean_rating,
            low_rating_cutoff: low_rating_cutoff(rated),
            rated_ids: rated.iter().map(|r| r.movie.id).collect(),
        }
    }

    /// Whether `rating` is among this user's lowest [`LOW_RATING_SHARE`] of ratings:
    /// strictly below their rating at that position, so ties at the cutoff don't
    /// count and the share is never much more than 40%. Relative to the user, so a
    /// generous rater's 6/10 can be low.
    pub fn is_low_rating(&self, rating: f32) -> bool {
        rating < self.low_rating_cutoff
    }

    /// Ratings below this are low ratings (see [`TasteProfile::is_low_rating`]).
    pub fn low_rating_cutoff(&self) -> f32 {
        self.low_rating_cutoff
    }

    pub fn rated(&self) -> &[RatedVector] {
        &self.rated
    }

    /// The user's average rating, 0 when nothing is rated.
    pub fn mean_rating(&self) -> f32 {
        self.mean_rating
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

/// The rating at position `ceil(LOW_RATING_SHARE × n)` in ascending order. Ratings
/// strictly below it are the user's low ratings. 0 (nothing is low) when unrated.
fn low_rating_cutoff(rated: &[RatedDetails]) -> f32 {
    let mut ratings: Vec<f32> = rated.iter().map(|r| r.rating).collect();
    ratings.sort_by(f32::total_cmp);
    let position = (LOW_RATING_SHARE * ratings.len() as f32).ceil() as usize;
    ratings
        .get(position.min(ratings.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0.0)
}
