//! Scores a candidate movie against a taste profile.

use crate::profile::TasteProfile;
use crate::tmdb::Movie;

/// How likely the user is to enjoy `movie`, from 0 (unlikely) to 100 (very likely).
///
/// Starts as simple genre overlap; keywords, cast, crew and plot
/// similarity can be layered in later.
pub fn score(profile: &TasteProfile, movie: &Movie) -> u8 {
    if movie.genre_ids.is_empty() {
        return 0;
    }
    let total: f32 = movie
        .genre_ids
        .iter()
        .map(|id| profile.genre_weights.get(id).copied().unwrap_or(0.0))
        .sum();
    let avg = total / movie.genre_ids.len() as f32;
    (avg * 100.0).round().clamp(0.0, 100.0) as u8
}
