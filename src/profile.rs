//! The user's liked movies and the taste profile built from them.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::features::{self, FieldWeights};
use crate::tfidf::{Idf, Vector};
use crate::tmdb::MovieDetails;

#[derive(Debug, Deserialize)]
struct LikedFile {
    movies: Vec<LikedMovie>,
}

/// A movie the user likes, as written in their liked-movies file.
#[derive(Debug, Clone, Deserialize)]
pub struct LikedMovie {
    pub title: String,
    pub year: Option<u16>,
}

pub fn load_liked_movies(path: &Path) -> Result<Vec<LikedMovie>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let file: LikedFile =
        toml::from_str(&text).with_context(|| format!("could not parse {}", path.display()))?;
    Ok(file.movies)
}

/// One liked movie, ready to compare against candidates.
#[derive(Debug, Clone)]
pub struct LikedVector {
    pub title: String,
    pub vector: Vector,
}

/// Each liked movie as its own TF-IDF vector, kept separate rather than averaged
/// so that eclectic tastes (say, horror and romantic comedy) don't blur together.
#[derive(Debug, Clone)]
pub struct TasteProfile {
    idf: Idf,
    weights: FieldWeights,
    liked: Vec<LikedVector>,
}

impl TasteProfile {
    pub fn new(liked: &[MovieDetails], idf: Idf, weights: FieldWeights) -> Self {
        let liked = liked
            .iter()
            .map(|movie| LikedVector {
                title: movie.title.clone(),
                vector: Vector::new(&features::extract(movie, &weights), &idf),
            })
            .collect();
        Self {
            idf,
            weights,
            liked,
        }
    }

    pub fn liked(&self) -> &[LikedVector] {
        &self.liked
    }

    /// `movie` as a TF-IDF vector comparable with the liked movies.
    pub fn vectorize(&self, movie: &MovieDetails) -> Vector {
        Vector::new(&features::extract(movie, &self.weights), &self.idf)
    }
}
