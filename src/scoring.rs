//! Scores candidate movies against a taste profile.
//!
//! A candidate's score is a predicted rating: the similarity-weighted average of
//! the user's ratings for the rated movies most similar to it (its "neighbours"),
//! chosen by similarity alone, whatever the user rated them.

use crate::features;
use crate::profile::{RatedVector, TasteProfile};
use crate::tfidf::Vector;
use crate::tmdb::MovieDetails;

/// How many of the most similar rated movies predict each candidate's rating.
pub const NEIGHBOURS: usize = 20;

/// How much the user's average rating counts in the prediction, in the same units
/// as similarity. With weak neighbours the prediction stays near that average
/// instead of trusting one faint match.
pub const PRIOR_WEIGHT: f32 = 2.0;

/// A closest-neighbour similarity at or above this counts as a real match, so the
/// neighbours get full weight. Below it their weight ramps down linearly, so one
/// trivial overlap can't move a score far from the user's average. Typical best
/// matches between different films are around 0.05 to 0.15.
pub const CONFIDENT_SIMILARITY: f32 = 0.08;

/// Weight multiplier for neighbours among the user's low ratings (their bottom
/// 40%, see [`TasteProfile::is_low_rating`]), so a low rating moves a prediction 30%
/// less than an equally similar higher one.
pub const LOW_RATING_IMPACT: f32 = 0.7;

/// How many shared features [`Match::reasons`] lists.
const MAX_REASONS: usize = 3;

/// A candidate's score and why it got it.
#[derive(Debug, Clone)]
pub struct Match<'a> {
    /// Predicted rating × 10: 0 to 100. Candidates sharing nothing with any rated
    /// movie get the user's average rating × 10.
    pub score: u8,
    /// The most similar liked neighbour, if any.
    pub closest_liked: Option<&'a RatedVector>,
    /// The most similar disliked neighbour, if any. This pulled the score down.
    pub closest_disliked: Option<&'a RatedVector>,
    /// Features shared with `closest_liked`, strongest first, e.g. `director Denis Villeneuve`.
    pub reasons: Vec<String>,
}

/// How likely the user is to enjoy `movie`, from 0 (unlikely) to 100 (very likely).
pub fn score(profile: &TasteProfile, movie: &MovieDetails) -> u8 {
    score_detailed(profile, movie).score
}

/// One rated movie among a candidate's nearest neighbours.
#[derive(Debug, Clone)]
pub struct Neighbour<'a> {
    pub rated: &'a RatedVector,
    /// Cosine similarity to the candidate, 0.0 to 1.0.
    pub similarity: f32,
    /// `similarity` divided by the closest neighbour's similarity, times the
    /// prediction's confidence, times [`LOW_RATING_IMPACT`] for low ratings. With a
    /// confident match the closest counts 1.0 (0.7 if a low rating) and the rest count
    /// in proportion.
    pub weight: f32,
}

/// The rating predicted for one candidate, with the neighbours behind it.
#[derive(Debug, Clone)]
pub struct Prediction<'a> {
    /// Most similar first, at most [`NEIGHBOURS`].
    pub neighbours: Vec<Neighbour<'a>>,
    /// How much the user's average rating counted, in neighbour-weight units.
    pub prior_weight: f32,
    /// `min(1, closest similarity / CONFIDENT_SIMILARITY)`: how much the neighbours
    /// are trusted at all.
    pub confidence: f32,
    /// Predicted rating, 0 to 10 (0 when nothing is rated).
    pub rating: f32,
}

/// Predicts the user's rating for `movie` from its own [`NEIGHBOURS`] most similar
/// rated movies, picked by similarity alone:
///
/// `(Σ weight × rating + prior × average rating) / (Σ weight + prior)`
///
/// Weights are similarities relative to the closest neighbour (see [`Neighbour::weight`]).
/// Raw similarities between different films are small (often under 0.1), so without
/// this the average rating would drown out the neighbours. How close that closest
/// neighbour is still matters in two ways:
/// - confidence: every weight is also scaled by `min(1, top / CONFIDENT_SIMILARITY)`,
///   so a near-zero match barely moves the score from the average;
/// - prior: `PRIOR_WEIGHT × (1 − top)`, so near-identical matches barely use the average.
///
/// Weights of neighbours among the user's low ratings are multiplied by [`LOW_RATING_IMPACT`].
pub fn predict<'a>(profile: &'a TasteProfile, candidate: &Vector) -> Prediction<'a> {
    if profile.rated().is_empty() {
        return Prediction {
            neighbours: Vec::new(),
            prior_weight: 0.0,
            confidence: 0.0,
            rating: 0.0,
        };
    }
    let mut found: Vec<(&RatedVector, f32)> = profile
        .rated()
        .iter()
        .map(|r| (r, candidate.cosine(&r.vector)))
        .filter(|(_, similarity)| *similarity > 0.0)
        .collect();
    // Stable sort: ties keep the earlier rated movie.
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    found.truncate(NEIGHBOURS);

    let top = found.first().map_or(0.0, |(_, s)| *s);
    let confidence = (top / CONFIDENT_SIMILARITY).min(1.0);
    let neighbours: Vec<Neighbour> = found
        .into_iter()
        .map(|(rated, similarity)| {
            let impact = if profile.is_low_rating(rated.rating) {
                LOW_RATING_IMPACT
            } else {
                1.0
            };
            Neighbour {
                rated,
                similarity,
                weight: similarity / top * confidence * impact,
            }
        })
        .collect();

    // With no neighbours, top is 0 and the prediction is exactly the average.
    let prior_weight = PRIOR_WEIGHT * (1.0 - top);
    let total: f32 = neighbours.iter().map(|n| n.weight).sum();
    let weighted: f32 = neighbours.iter().map(|n| n.weight * n.rated.rating).sum();
    let rating = (weighted + prior_weight * profile.mean_rating()) / (total + prior_weight);
    Prediction {
        neighbours,
        prior_weight,
        confidence,
        rating,
    }
}

pub fn score_detailed<'a>(profile: &'a TasteProfile, movie: &MovieDetails) -> Match<'a> {
    let candidate = profile.vectorize(movie);
    let prediction = predict(profile, &candidate);

    let mut rated = prediction.neighbours.iter().map(|n| n.rated);
    let closest_liked = rated.clone().find(|r| r.is_liked());
    let closest_disliked = rated.find(|r| r.is_disliked());
    let reasons = match closest_liked {
        Some(liked) => reasons(profile, movie, &candidate, &liked.vector),
        None => Vec::new(),
    };
    Match {
        score: to_score(prediction.rating),
        closest_liked,
        closest_disliked,
        reasons,
    }
}

/// A candidate's prediction spelled out neighbour by neighbour, for `--explain`.
#[derive(Debug, Clone)]
pub struct Explanation {
    pub score: u8,
    pub predicted_rating: f32,
    pub mean_rating: f32,
    /// How much `mean_rating` counted, in neighbour-weight units.
    pub prior_weight: f32,
    /// How much the neighbours were trusted, 0.0 to 1.0 (see [`Prediction::confidence`]).
    pub confidence: f32,
    /// Ratings below this are the user's low ratings and count 30% less.
    pub low_rating_cutoff: f32,
    pub neighbours: Vec<ExplainedNeighbour>,
}

#[derive(Debug, Clone)]
pub struct ExplainedNeighbour {
    pub title: String,
    pub rating: f32,
    pub similarity: f32,
    pub weight: f32,
    /// Features shared with the candidate, strongest first.
    pub shared: Vec<String>,
}

pub fn explain(profile: &TasteProfile, movie: &MovieDetails) -> Explanation {
    let candidate = profile.vectorize(movie);
    let prediction = predict(profile, &candidate);
    let labels = features::labels(movie, profile.weights());
    let neighbours = prediction
        .neighbours
        .iter()
        .map(|n| ExplainedNeighbour {
            title: n.rated.title.clone(),
            rating: n.rated.rating,
            similarity: n.similarity,
            weight: n.weight,
            shared: candidate
                .shared(&n.rated.vector)
                .into_iter()
                .filter_map(|(token, _)| labels.get(token).cloned())
                .collect(),
        })
        .collect();
    Explanation {
        score: to_score(prediction.rating),
        predicted_rating: prediction.rating,
        mean_rating: profile.mean_rating(),
        prior_weight: prediction.prior_weight,
        confidence: prediction.confidence,
        low_rating_cutoff: profile.low_rating_cutoff(),
        neighbours,
    }
}

fn to_score(rating: f32) -> u8 {
    (rating * 10.0).round().clamp(0.0, 100.0) as u8
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
