//! The user's liked movies and the taste profile built from them.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::tmdb::Movie;

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
    let file: LikedFile = toml::from_str(&text)?;
    Ok(file.movies)
}

/// Aggregated preferences built from the TMDB details of liked movies.
#[derive(Debug, Default)]
pub struct TasteProfile {
    /// Fraction of liked movies that have each TMDB genre id (0.0 to 1.0).
    pub genre_weights: HashMap<u32, f32>,
}

impl TasteProfile {
    pub fn from_movies(movies: &[Movie]) -> Self {
        let mut counts: HashMap<u32, f32> = HashMap::new();
        for movie in movies {
            for &id in &movie.genre_ids {
                *counts.entry(id).or_default() += 1.0;
            }
        }
        let n = movies.len().max(1) as f32;
        let genre_weights = counts.into_iter().map(|(id, c)| (id, c / n)).collect();
        Self { genre_weights }
    }
}
