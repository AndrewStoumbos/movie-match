//! Scores candidate movies against a taste profile.
//!
//! A candidate's score is a predicted rating, from two formulas blended by how many
//! TMDB audience votes the film has (see [`blend`]):
//!
//! - **Unreleased** (no votes yet): the similarity rating, an average of the user's
//!   ratings for the rated movies most similar to it (its "neighbours"), chosen by
//!   similarity alone, whatever the user rated them. Closer neighbours count more
//!   (by similarity squared), and the average leans toward the user's overall
//!   average only when there are few neighbours. Then nudged by how far its cast and
//!   crew's track record sits from the typical film's ([`TRACK_RECORD_NUDGE`]).
//! - **Released**: the audience score plus the user's usual difference from the
//!   audience for films like it ([`AudienceShift`]).

use std::collections::HashMap;

use crate::features;
use crate::profile::{RatedVector, TasteProfile};
use crate::shift::{AudienceShift, ShiftPrediction};
use crate::tfidf::Vector;
use crate::tmdb::MovieDetails;

/// How many of the most similar rated movies predict each candidate's rating.
pub const NEIGHBOURS: usize = 20;

/// How sharply closer neighbours outweigh further ones: each neighbour's weight is
/// `(similarity / closest similarity) ^ SIMILARITY_POWER`. With 2, a neighbour half as
/// similar as the closest counts a quarter as much.
pub const SIMILARITY_POWER: i32 = 2;

/// How many ratings at the user's overall average are blended into the neighbours'
/// weighted average:
/// `(neighbours × weighted average + OVERALL_SHRINK × overall average) / (neighbours + OVERALL_SHRINK)`.
/// With the full 20 neighbours the overall average is a fifth of the prediction; with
/// one neighbour it's most of it, so a single overlap can't decide a score.
pub const OVERALL_SHRINK: f32 = 5.0;

/// How many shared features [`Match::reasons`] lists.
const MAX_REASONS: usize = 3;

/// Audience votes at which a film's score is half the released formula and half
/// the unreleased one: the released formula's share is `votes / (votes +
/// AUDIENCE_VOTES_HALF)`, so a festival film with 10 votes is still mostly the
/// unreleased formula and one with 5,000 is almost all the released one.
pub const AUDIENCE_VOTES_HALF: f32 = 100.0;

/// How far a film's track record moves its unreleased score: by `TRACK_RECORD_NUDGE ×
/// (estimate − typical)` rating points. 1.0 predicted best of the strengths tried
/// on MovieLens users that tuning never saw.
pub const TRACK_RECORD_NUDGE: f32 = 1.0;

/// What scoring can use beyond the user's ratings. Without either part (the
/// default), every film gets its similarity rating alone.
#[derive(Debug, Clone, Default)]
pub struct Signals {
    /// The user's difference from the audience; `None` when none of their rated
    /// movies has audience votes, and then the released formula isn't used.
    pub shift: Option<AudienceShift>,
    /// Cast and crew track records for the candidates.
    pub track_records: Option<TrackRecords>,
}

/// Estimated audience scores from cast and crew track records (see
/// [`crate::quality::estimate`]).
#[derive(Debug, Clone, Default)]
pub struct TrackRecords {
    /// What a film is expected to score knowing nothing else.
    pub typical: f32,
    /// Estimates per TMDB id. A film without one isn't nudged.
    pub estimates: HashMap<u64, f32>,
}

/// How a film's score was put together.
#[derive(Debug, Clone, PartialEq)]
pub struct Breakdown {
    /// The similarity rating, 0 to 10 (see [`predict`]).
    pub similarity: f32,
    /// The track-record nudge, when the film has an estimate.
    pub track_record: Option<TrackRecordNudge>,
    /// The unreleased formula: `similarity` plus the nudge, 0 to 10.
    pub unreleased: f32,
    /// The released formula, when the film has audience votes and the user a shift.
    pub audience: Option<AudienceView>,
    /// How much of `rating` is the released formula, 0 to 1 (see [`blend`]).
    pub audience_share: f32,
    /// The predicted rating, 0 to 10.
    pub rating: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackRecordNudge {
    /// The film's estimated audience score from its cast and crew's earlier films.
    pub estimate: f32,
    pub typical: f32,
    /// What the nudge adds: `TRACK_RECORD_NUDGE × (estimate − typical)`.
    pub nudge: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudienceView {
    /// TMDB audience score, 0 to 10.
    pub score: f32,
    pub votes: u32,
    /// The user's predicted difference from that score.
    pub shift: ShiftPrediction,
    /// `score + shift`, 0 to 10.
    pub rating: f32,
}

/// The released formula's share of a film's score: `votes / (votes + AUDIENCE_VOTES_HALF)`.
pub fn audience_share(votes: u32) -> f32 {
    let votes = votes as f32;
    votes / (votes + AUDIENCE_VOTES_HALF)
}

/// The two formulas blended: `share × released + (1 − share) × unreleased`, with
/// `share` from [`audience_share`], or just `unreleased` without a released rating.
/// Returns the rating and the share.
pub fn blend(unreleased: f32, released: Option<f32>, votes: u32) -> (f32, f32) {
    match released {
        Some(released) => {
            let share = audience_share(votes);
            (share * released + (1.0 - share) * unreleased, share)
        }
        None => (unreleased, 0.0),
    }
}

/// `similarity` nudged by a track-record estimate, 0 to 10.
pub fn nudged(similarity: f32, estimate: Option<f32>, typical: f32) -> f32 {
    let nudge = estimate.map_or(0.0, |e| TRACK_RECORD_NUDGE * (e - typical));
    (similarity + nudge).clamp(0.0, 10.0)
}

/// Puts `movie`'s score together from its similarity rating and `signals`.
pub fn breakdown(similarity: f32, movie: &MovieDetails, signals: &Signals) -> Breakdown {
    let track_record = signals.track_records.as_ref().and_then(|records| {
        let estimate = *records.estimates.get(&movie.id)?;
        Some(TrackRecordNudge {
            estimate,
            typical: records.typical,
            nudge: TRACK_RECORD_NUDGE * (estimate - records.typical),
        })
    });
    let unreleased = nudged(
        similarity,
        track_record.as_ref().map(|t| t.estimate),
        track_record.as_ref().map_or(0.0, |t| t.typical),
    );
    let audience = match (&signals.shift, movie.audience_rating()) {
        (Some(shift), Some(score)) => {
            let shift = shift.predict(movie);
            Some(AudienceView {
                score,
                votes: movie.vote_count.unwrap_or(0),
                rating: (score + shift.total()).clamp(0.0, 10.0),
                shift,
            })
        }
        _ => None,
    };
    let (rating, audience_share) = blend(
        unreleased,
        audience.as_ref().map(|a| a.rating),
        audience.as_ref().map_or(0, |a| a.votes),
    );
    Breakdown {
        similarity,
        track_record,
        unreleased,
        audience,
        audience_share,
        rating,
    }
}

/// A candidate's score and why it got it.
#[derive(Debug, Clone)]
pub struct Match<'a> {
    /// Predicted rating × 10: 0 to 100. Candidates sharing nothing with any rated
    /// movie get the user's overall average rating × 10.
    pub score: u8,
    /// The most similar liked neighbour, if any.
    pub closest_liked: Option<&'a RatedVector>,
    /// The most similar disliked neighbour, if any. This pulled the score down.
    pub closest_disliked: Option<&'a RatedVector>,
    /// Features shared with `closest_liked`, strongest first, e.g. `director Denis Villeneuve`.
    pub reasons: Vec<String>,
    pub breakdown: Breakdown,
}

/// How likely the user is to enjoy `movie`, from 0 (unlikely) to 100 (very likely),
/// from similarity alone.
pub fn score(profile: &TasteProfile, movie: &MovieDetails) -> u8 {
    score_detailed(profile, movie).score
}

/// One rated movie among a candidate's nearest neighbours.
#[derive(Debug, Clone)]
pub struct Neighbour<'a> {
    pub rated: &'a RatedVector,
    /// Cosine similarity to the candidate, 0.0 to 1.0.
    pub similarity: f32,
    /// `(similarity / closest similarity) ^ SIMILARITY_POWER`: the closest counts
    /// 1.0, one half as similar counts 0.25. Movies tied for the last of the
    /// [`NEIGHBOURS`] places split the places left: 30 tied for 10 places count a
    /// third each.
    pub weight: f32,
}

/// The rating predicted for one candidate, with the neighbours behind it.
#[derive(Debug, Clone)]
pub struct Prediction<'a> {
    /// Most similar first: [`NEIGHBOURS`] of them, or more when several are tied for
    /// the last place (all of those are kept, see [`Neighbour::weight`]).
    pub neighbours: Vec<Neighbour<'a>>,
    /// The neighbours' ratings averaged by [`Neighbour::weight`] (0 with no neighbours).
    pub neighbour_rating: f32,
    /// How much of the prediction is the user's overall average, 0.0 to 1.0:
    /// `OVERALL_SHRINK / (neighbours + OVERALL_SHRINK)`, so 1.0 with no neighbours,
    /// counting at most [`NEIGHBOURS`] neighbours.
    pub overall_share: f32,
    /// Predicted rating, 0 to 10 (0 when nothing is rated).
    pub rating: f32,
}

/// Predicts the user's rating for `movie` from its own [`NEIGHBOURS`] most similar
/// rated movies, picked by similarity alone:
///
/// `weighted = Σ weight × rating / Σ weight`, with each weight
/// `(similarity / closest) ^ SIMILARITY_POWER` (see [`Neighbour::weight`]), then
///
/// `(n × weighted + OVERALL_SHRINK × overall average) / (n + OVERALL_SHRINK)`
///
/// for `n` neighbours. So a movie is predicted from how the user rated the movies
/// most like it, the closest counting far more, not from everything they've rated;
/// only with few neighbours does it lean on the overall average, and with none it's
/// exactly that average. Weights are relative to the closest neighbour because raw
/// similarities between different films are small (often under 0.1) and only their
/// ratios matter. Movies tied for the last place are all kept and split
/// it, so the order of the ratings file never changes a prediction.
pub fn predict<'a>(profile: &'a TasteProfile, candidate: &Vector) -> Prediction<'a> {
    if profile.rated().is_empty() {
        return Prediction {
            neighbours: Vec::new(),
            neighbour_rating: 0.0,
            overall_share: 0.0,
            rating: 0.0,
        };
    }
    let mut found: Vec<(&RatedVector, f32)> = profile
        .rated()
        .iter()
        .map(|r| (r, candidate.cosine(&r.vector)))
        .filter(|(_, similarity)| *similarity > 0.0)
        .collect();
    // Stable sort: ties keep the earlier rated movie, which only orders the list.
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    // Movies tied with the last neighbour all stay and share its remaining slots, so
    // the prediction never depends on the order of the ratings file.
    let mut tie_share = 1.0;
    let mut cutoff = f32::NEG_INFINITY;
    if found.len() > NEIGHBOURS {
        cutoff = found[NEIGHBOURS - 1].1;
        let above = found.iter().take_while(|(_, s)| *s > cutoff).count();
        let kept = found.iter().take_while(|(_, s)| *s >= cutoff).count();
        tie_share = (NEIGHBOURS - above) as f32 / (kept - above) as f32;
        found.truncate(kept);
    }

    let top = found.first().map_or(0.0, |(_, s)| *s);
    let neighbours: Vec<Neighbour> = found
        .into_iter()
        .map(|(rated, similarity)| {
            let share = if similarity == cutoff { tie_share } else { 1.0 };
            Neighbour {
                rated,
                similarity,
                weight: (similarity / top).powi(SIMILARITY_POWER) * share,
            }
        })
        .collect();

    let total: f32 = neighbours.iter().map(|n| n.weight).sum();
    let weighted: f32 = neighbours.iter().map(|n| n.weight * n.rated.rating).sum();
    let neighbour_rating = if total > 0.0 { weighted / total } else { 0.0 };
    // With no neighbours the share is 1 and the prediction is exactly the overall average.
    let n = neighbours.len().min(NEIGHBOURS) as f32;
    let overall_share = OVERALL_SHRINK / (n + OVERALL_SHRINK);
    let rating = (1.0 - overall_share) * neighbour_rating + overall_share * profile.mean_rating();
    Prediction {
        neighbours,
        neighbour_rating,
        overall_share,
        rating,
    }
}

/// [`score_detailed_with`] from similarity alone.
pub fn score_detailed<'a>(profile: &'a TasteProfile, movie: &MovieDetails) -> Match<'a> {
    score_detailed_with(profile, &Signals::default(), movie)
}

pub fn score_detailed_with<'a>(
    profile: &'a TasteProfile,
    signals: &Signals,
    movie: &MovieDetails,
) -> Match<'a> {
    let candidate = profile.vectorize(movie);
    let prediction = predict(profile, &candidate);

    let mut rated = prediction.neighbours.iter().map(|n| n.rated);
    let closest_liked = rated.clone().find(|r| r.is_liked());
    let closest_disliked = rated.find(|r| r.is_disliked());
    let reasons = match closest_liked {
        Some(liked) => reasons(profile, movie, &candidate, &liked.vector),
        None => Vec::new(),
    };
    let breakdown = breakdown(prediction.rating, movie, signals);
    Match {
        score: to_score(breakdown.rating),
        closest_liked,
        closest_disliked,
        reasons,
        breakdown,
    }
}

/// A candidate's prediction spelled out neighbour by neighbour, for `--explain`.
#[derive(Debug, Clone)]
pub struct Explanation {
    pub score: u8,
    pub predicted_rating: f32,
    /// How the similarity rating, track record and audience combine.
    pub breakdown: Breakdown,
    /// The user's overall average rating.
    pub mean_rating: f32,
    /// The neighbours' ratings averaged by weight (see [`Prediction::neighbour_rating`]).
    pub neighbour_rating: f32,
    /// How much of the prediction is `mean_rating`, 0.0 to 1.0 (see
    /// [`Prediction::overall_share`]).
    pub overall_share: f32,
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

/// [`explain_with`] from similarity alone.
pub fn explain(profile: &TasteProfile, movie: &MovieDetails) -> Explanation {
    explain_with(profile, &Signals::default(), movie)
}

pub fn explain_with(
    profile: &TasteProfile,
    signals: &Signals,
    movie: &MovieDetails,
) -> Explanation {
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
    let breakdown = breakdown(prediction.rating, movie, signals);
    Explanation {
        score: to_score(breakdown.rating),
        predicted_rating: breakdown.rating,
        breakdown,
        mean_rating: profile.mean_rating(),
        neighbour_rating: prediction.neighbour_rating,
        overall_share: prediction.overall_share,
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
    pub breakdown: Breakdown,
}

/// [`rank_with`] from similarity alone.
pub fn rank(profile: &TasteProfile, candidates: &[MovieDetails]) -> Vec<Ranked> {
    rank_with(profile, &Signals::default(), candidates)
}

/// Scores every candidate the user hasn't already rated, best match first
/// (ties broken by title).
pub fn rank_with(
    profile: &TasteProfile,
    signals: &Signals,
    candidates: &[MovieDetails],
) -> Vec<Ranked> {
    let summary = |r: &RatedVector| (r.title.clone(), r.rating);
    let mut ranked: Vec<Ranked> = candidates
        .iter()
        .filter(|movie| !profile.has_rated(movie.id))
        .map(|movie| {
            let found = score_detailed_with(profile, signals, movie);
            Ranked {
                movie: movie.clone(),
                score: found.score,
                closest_liked: found.closest_liked.map(summary),
                closest_disliked: found.closest_disliked.map(summary),
                reasons: found.reasons,
                breakdown: found.breakdown,
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
