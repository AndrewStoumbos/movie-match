//! Scores candidate movies against a taste profile.

use crate::features;
use crate::profile::{RatedVector, TasteProfile};
use crate::tfidf::Vector;
use crate::tmdb::MovieDetails;

/// How much resembling a disliked movie counts against a candidate, relative to
/// how much resembling a liked one counts for it.
pub const DISLIKE_WEIGHT: f32 = 0.5;

/// How many shared features [`Match::reasons`] lists.
const MAX_REASONS: usize = 3;

/// A candidate's score and why it got it.
#[derive(Debug, Clone)]
pub struct Match<'a> {
    /// 0 (unlikely to enjoy) to 100 (identical to a 10/10 favourite).
    pub score: u8,
    /// The liked movie the candidate most resembles, if it shares anything with one.
    pub closest_liked: Option<&'a RatedVector>,
    /// The disliked movie it most resembles, if any. This is what lowered the score.
    pub closest_disliked: Option<&'a RatedVector>,
    /// Features shared with `closest_liked`, strongest first, e.g. `director Denis Villeneuve`.
    pub reasons: Vec<String>,
}

/// How likely the user is to enjoy `movie`, from 0 (unlikely) to 100 (very likely).
pub fn score(profile: &TasteProfile, movie: &MovieDetails) -> u8 {
    score_detailed(profile, movie).score
}

/// Field-weighted TF-IDF similarity to the closest liked movie (scaled by its
/// rating), minus [`DISLIKE_WEIGHT`] × similarity to the closest disliked one.
pub fn score_detailed<'a>(profile: &'a TasteProfile, movie: &MovieDetails) -> Match<'a> {
    let candidate = profile.vectorize(movie);
    let liked = closest(profile.liked(), &candidate);
    let disliked = closest(profile.disliked(), &candidate);

    let pull = liked.map_or(0.0, |(_, value)| value);
    let push = disliked.map_or(0.0, |(_, value)| value);
    let value = (pull - DISLIKE_WEIGHT * push).clamp(0.0, 1.0);

    let reasons = match liked {
        Some((liked, _)) => reasons(profile, movie, &candidate, &liked.vector),
        None => Vec::new(),
    };
    Match {
        score: (value * 100.0).round() as u8,
        closest_liked: liked.map(|(r, _)| r),
        closest_disliked: disliked.map(|(r, _)| r),
        reasons,
    }
}

/// A scored candidate that doesn't borrow from the profile.
#[derive(Debug, Clone)]
pub struct Ranked {
    pub movie: MovieDetails,
    pub score: u8,
    /// Title and rating of the closest liked movie.
    pub closest_liked: Option<(String, f32)>,
    /// Title and rating of the closest disliked movie.
    pub closest_disliked: Option<(String, f32)>,
    pub reasons: Vec<String>,
}

/// Scores every candidate the user hasn't already rated, best match first
/// (ties broken by title).
pub fn rank(profile: &TasteProfile, candidates: &[MovieDetails]) -> Vec<Ranked> {
    let summary = |r: &RatedVector| (r.title.clone(), r.rating);
    let mut ranked: Vec<Ranked> = candidates
        .iter()
        .filter(|movie| !profile.has_rated(movie.id))
        .map(|movie| {
            let found = score_detailed(profile, movie);
            Ranked {
                movie: movie.clone(),
                score: found.score,
                closest_liked: found.closest_liked.map(summary),
                closest_disliked: found.closest_disliked.map(summary),
                reasons: found.reasons,
            }
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.movie.title.cmp(&b.movie.title))
    });
    ranked
}

/// The movie in `rated` with the highest similarity × strength, if any is above zero.
/// Ties keep the earlier movie.
fn closest<'a>(rated: &'a [RatedVector], candidate: &Vector) -> Option<(&'a RatedVector, f32)> {
    let mut best: Option<(&RatedVector, f32)> = None;
    for r in rated {
        let value = candidate.cosine(&r.vector) * r.strength;
        if value > best.map_or(0.0, |(_, v)| v) {
            best = Some((r, value));
        }
    }
    best
}

fn reasons(
    profile: &TasteProfile,
    movie: &MovieDetails,
    candidate: &Vector,
    liked: &Vector,
) -> Vec<String> {
    let labels = features::labels(movie, profile.weights());
    candidate
        .shared(liked)
        .into_iter()
        .filter_map(|(token, _)| labels.get(token).cloned())
        .take(MAX_REASONS)
        .collect()
}
