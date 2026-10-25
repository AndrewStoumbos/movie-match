//! Scores a candidate movie against a taste profile.

use crate::profile::TasteProfile;
use crate::tmdb::MovieDetails;

/// A candidate's score and the liked movie it most resembles.
#[derive(Debug, Clone, PartialEq)]
pub struct Match<'a> {
    /// 0 (nothing in common) to 100 (identical features).
    pub score: u8,
    /// Title of the most similar liked movie, if any shares a feature.
    pub closest_liked: Option<&'a str>,
}

/// How likely the user is to enjoy `movie`, from 0 (unlikely) to 100 (very likely).
pub fn score(profile: &TasteProfile, movie: &MovieDetails) -> u8 {
    score_detailed(profile, movie).score
}

/// Field-weighted TF-IDF cosine similarity to the single closest liked movie.
pub fn score_detailed<'a>(profile: &'a TasteProfile, movie: &MovieDetails) -> Match<'a> {
    let candidate = profile.vectorize(movie);
    let mut best: Option<(&str, f32)> = None;
    for liked in profile.liked() {
        let similarity = candidate.cosine(&liked.vector);
        // Strictly greater, so ties keep the earlier liked movie.
        if similarity > best.map_or(0.0, |(_, s)| s) {
            best = Some((liked.title.as_str(), similarity));
        }
    }
    match best {
        Some((title, similarity)) => Match {
            score: (similarity * 100.0).round().clamp(0.0, 100.0) as u8,
            closest_liked: Some(title),
        },
        None => Match {
            score: 0,
            closest_liked: None,
        },
    }
}
