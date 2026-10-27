//! Pre-release quality estimates: a film's likely TMDB audience score from the
//! track records of the people who made it, using only films they made before it.
//! Pure: callers supply the films' details and the people's filmographies.

use std::collections::{HashMap, HashSet};

use crate::evaluate::spearman;
use crate::features::FieldWeights;
use crate::tmdb::{CreditedFilm, MovieDetails};

// The three settings below were tuned with `--evaluate-track-record` on half of a
// 1,000-film MovieLens sample (857 with enough votes) and checked on the other half:
// track records are worth trusting fully, with only low-vote films held back.

/// Votes' worth of the typical score blended into each earlier film's audience
/// score, so a film with a handful of votes barely moves a track record (the idea
/// behind IMDb's weighted rating).
pub const PRIOR_VOTES: f32 = 30.0;

/// Earlier films' worth of "no information" in each person's weight; with n earlier
/// films a person counts `n / (n + PERSON_SHRINK)` of their role's weight. 0: someone
/// with one earlier film counts as much as someone with many.
pub const PERSON_SHRINK: f32 = 0.0;

/// How much the typical score counts in an estimate, in field-weight units (a
/// director counts 3). 0: the estimate is the track records' weighted average alone.
pub const ESTIMATE_SHRINK: f32 = 0.0;

/// Films need at least this many votes to count toward the typical score.
pub const TYPICAL_MIN_VOTES: u32 = 100;

/// Films need at least this many votes to serve as the true score in an evaluation.
pub const MIN_TRUTH_VOTES: u32 = 100;

/// The three cautions in [`estimate`], adjustable so they can be tuned. The default
/// is [`PRIOR_VOTES`], [`PERSON_SHRINK`] and [`ESTIMATE_SHRINK`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EstimateSettings {
    pub prior_votes: f32,
    pub person_shrink: f32,
    pub estimate_shrink: f32,
}

impl Default for EstimateSettings {
    fn default() -> Self {
        Self {
            prior_votes: PRIOR_VOTES,
            person_shrink: PERSON_SHRINK,
            estimate_shrink: ESTIMATE_SHRINK,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Director,
    Writer,
    Cast,
}

/// One person whose record counts toward a film, with their role's weight.
#[derive(Debug, Clone, PartialEq)]
pub struct Credit {
    pub person: u64,
    pub role: Role,
    pub weight: f32,
}

/// The people whose records count for `movie`: directors and writers at their field
/// weights, and the top `cast_limit` billed actors at `cast / billing position`.
pub fn credits(movie: &MovieDetails, weights: &FieldWeights, cast_limit: usize) -> Vec<Credit> {
    let mut credits = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |person: u64, role: Role, weight: f32| {
        if seen.insert((person, role as u8)) {
            credits.push(Credit {
                person,
                role,
                weight,
            });
        }
    };
    for person in movie.crew_with_job("Director") {
        add(person.id, Role::Director, weights.director);
    }
    for job in ["Screenplay", "Writer"] {
        for person in movie.crew_with_job(job) {
            add(person.id, Role::Writer, weights.writer);
        }
    }
    let mut cast: Vec<_> = movie.credits.cast.iter().collect();
    cast.sort_by_key(|actor| actor.order);
    for (i, actor) in cast.iter().take(cast_limit).enumerate() {
        add(actor.id, Role::Cast, weights.cast / (i as f32 + 1.0));
    }
    credits
}

/// The average audience score of every distinct film in `filmographies` with at
/// least [`TYPICAL_MIN_VOTES`] votes: what a film is expected to score knowing nothing
/// else. `None` if there are none.
pub fn typical_score(filmographies: &HashMap<u64, Vec<CreditedFilm>>) -> Option<f32> {
    let mut seen = HashSet::new();
    let scores: Vec<f32> = filmographies
        .values()
        .flatten()
        .filter(|film| film.votes() >= TYPICAL_MIN_VOTES && seen.insert(film.id))
        .filter_map(|film| film.vote_average)
        .collect();
    (!scores.is_empty()).then(|| scores.iter().sum::<f32>() / scores.len() as f32)
}

/// `movie`'s estimated audience score from the films its `credits` made before it:
///
/// - each earlier film counts as `(votes × score + PRIOR_VOTES × typical) / (votes + PRIOR_VOTES)`;
/// - a person's record is the average of those, weighted by their credit's weight
///   × `n / (n + PERSON_SHRINK)` for their `n` earlier films;
/// - the estimate is the weighted average of the records, with `typical` counting
///   [`ESTIMATE_SHRINK`].
///
/// Films released on or after `movie` (and `movie` itself) are ignored, so this is
/// what could have been known before release. A person credited twice counts once,
/// at their highest weight. `None` without a release date or any earlier films.
pub fn estimate(
    movie: &MovieDetails,
    credits: &[Credit],
    filmographies: &HashMap<u64, Vec<CreditedFilm>>,
    typical: f32,
) -> Option<f32> {
    estimate_with(
        movie,
        credits,
        filmographies,
        typical,
        &EstimateSettings::default(),
    )
}

/// [`estimate`] with its three cautions set by `settings`.
pub fn estimate_with(
    movie: &MovieDetails,
    credits: &[Credit],
    filmographies: &HashMap<u64, Vec<CreditedFilm>>,
    typical: f32,
    settings: &EstimateSettings,
) -> Option<f32> {
    let EstimateSettings {
        prior_votes,
        person_shrink,
        estimate_shrink,
    } = *settings;
    if movie.release_date.is_empty() {
        return None;
    }
    let mut people: Vec<(u64, f32)> = Vec::new();
    for credit in credits {
        match people.iter_mut().find(|(p, _)| *p == credit.person) {
            Some((_, weight)) => *weight = weight.max(credit.weight),
            None => people.push((credit.person, credit.weight)),
        }
    }
    let (mut total, mut weighted) = (0.0, 0.0);
    for (person, weight) in people {
        let Some(films) = filmographies.get(&person) else {
            continue;
        };
        let earlier: Vec<f32> = films
            .iter()
            .filter(|film| film.id != movie.id)
            .filter(|film| {
                film.released()
                    .is_some_and(|d| d < movie.release_date.as_str())
            })
            .filter_map(|film| {
                let votes = film.votes() as f32;
                let score = film.vote_average?;
                Some((votes * score + prior_votes * typical) / (votes + prior_votes))
            })
            .collect();
        if earlier.is_empty() {
            continue;
        }
        let n = earlier.len() as f32;
        let record = earlier.iter().sum::<f32>() / n;
        let person_weight = weight * n / (n + person_shrink);
        total += person_weight;
        weighted += person_weight * record;
    }
    (total > 0.0).then(|| (weighted + estimate_shrink * typical) / (total + estimate_shrink))
}

/// The estimators compared, in report order: the typical score for every film (what
/// an unreleased film gets today), then track records of each role and all together.
pub const TRACK_RECORD_METHODS: [&str; 5] = [
    "typical score",
    "directors' records",
    "writers' records",
    "top cast's records",
    "everyone's records",
];

#[derive(Debug, Clone)]
pub struct TrackRecordResult {
    pub name: &'static str,
    /// Share of films (0 to 1) the method could estimate; the rest get the typical score.
    pub coverage: f32,
    /// Mean absolute error against the real audience score, 0 to 10.
    pub mae: f32,
    pub rmse: f32,
    /// Spearman correlation with the real scores: how well it orders the films.
    /// `None` for the typical score, which is the same for every film.
    pub spearman: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct TrackRecordReport {
    /// Films with at least [`MIN_TRUTH_VOTES`] votes, so their real score is reliable.
    pub films: usize,
    pub typical: f32,
    /// One per [`TRACK_RECORD_METHODS`] item.
    pub results: Vec<TrackRecordResult>,
}

/// How well each [`TRACK_RECORD_METHODS`] estimator predicts the real audience
/// score of `films` that have at least [`MIN_TRUTH_VOTES`] votes.
pub fn evaluate_track_record(
    films: &[MovieDetails],
    filmographies: &HashMap<u64, Vec<CreditedFilm>>,
    weights: &FieldWeights,
    cast_limit: usize,
) -> TrackRecordReport {
    let typical = typical_score(filmographies).unwrap_or(0.0);
    let targets = targets(films, weights, cast_limit);
    let roles: [Option<&[Role]>; 5] = [
        Some(&[]),
        Some(&[Role::Director]),
        Some(&[Role::Writer]),
        Some(&[Role::Cast]),
        None,
    ];
    let settings = EstimateSettings::default();
    let results = TRACK_RECORD_METHODS
        .iter()
        .zip(roles)
        .map(|(&name, roles)| {
            let (coverage, scores) = score(&targets, filmographies, typical, &settings, roles);
            TrackRecordResult {
                name,
                coverage,
                mae: scores.mae,
                rmse: scores.rmse,
                spearman: scores.spearman,
            }
        })
        .collect();
    TrackRecordReport {
        films: targets.len(),
        typical,
        results,
    }
}

/// How estimates compare with the real audience scores.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scores {
    pub mae: f32,
    pub rmse: f32,
    /// `None` when every estimate is the same.
    pub spearman: Option<f32>,
}

/// The values tried for each [`EstimateSettings`] field when tuning.
pub const TUNING_PRIOR_VOTES: [f32; 5] = [10.0, 30.0, 100.0, 300.0, 1000.0];
pub const TUNING_PERSON_SHRINK: [f32; 4] = [0.0, 1.0, 3.0, 10.0];
pub const TUNING_ESTIMATE_SHRINK: [f32; 6] = [0.0, 0.25, 0.5, 1.0, 2.0, 4.0];

#[derive(Debug, Clone)]
pub struct TuningReport {
    pub tune_films: usize,
    pub check_films: usize,
    /// The combination with the lowest MAE on the tuning half.
    pub best: EstimateSettings,
    /// How `best` did on the tuning half itself: optimistic, since it was chosen there.
    pub best_on_tune_half: Scores,
    /// On the checking half, which tuning never saw: the typical score for every
    /// film, the default settings, and the tuned ones.
    pub check: [(&'static str, Scores); 3],
}

/// Tries every combination of [`TUNING_PRIOR_VOTES`], [`TUNING_PERSON_SHRINK`] and
/// [`TUNING_ESTIMATE_SHRINK`] for "everyone's records" on half of `films` (every
/// other film by TMDB id), keeps the one with the lowest MAE, and checks it on the
/// other half against the defaults. Ties keep the first combination tried.
pub fn tune(
    films: &[MovieDetails],
    filmographies: &HashMap<u64, Vec<CreditedFilm>>,
    weights: &FieldWeights,
    cast_limit: usize,
) -> TuningReport {
    let typical = typical_score(filmographies).unwrap_or(0.0);
    let all = targets(films, weights, cast_limit);
    let (tune_half, check_half): (Vec<_>, Vec<_>) =
        all.into_iter().enumerate().partition(|(i, _)| i % 2 == 0);
    let tune_half: Vec<Target> = tune_half.into_iter().map(|(_, t)| t).collect();
    let check_half: Vec<Target> = check_half.into_iter().map(|(_, t)| t).collect();

    let mut best: Option<(EstimateSettings, Scores)> = None;
    for prior_votes in TUNING_PRIOR_VOTES {
        for person_shrink in TUNING_PERSON_SHRINK {
            for estimate_shrink in TUNING_ESTIMATE_SHRINK {
                let settings = EstimateSettings {
                    prior_votes,
                    person_shrink,
                    estimate_shrink,
                };
                let (_, scores) = score(&tune_half, filmographies, typical, &settings, None);
                if best.is_none_or(|(_, b)| scores.mae < b.mae) {
                    best = Some((settings, scores));
                }
            }
        }
    }
    // The grid is never empty, but fall back to the defaults rather than panic.
    let (best, best_on_tune_half) = best.unwrap_or_else(|| {
        let settings = EstimateSettings::default();
        (
            settings,
            score(&tune_half, filmographies, typical, &settings, None).1,
        )
    });

    let on_check = |settings: &EstimateSettings, roles| {
        score(&check_half, filmographies, typical, settings, roles).1
    };
    TuningReport {
        tune_films: tune_half.len(),
        check_films: check_half.len(),
        best,
        best_on_tune_half,
        check: [
            (
                "typical score",
                on_check(&EstimateSettings::default(), Some(&[])),
            ),
            (
                "default settings",
                on_check(&EstimateSettings::default(), None),
            ),
            ("tuned settings", on_check(&best, None)),
        ],
    }
}

/// A film with a reliable real score, and the people whose records count for it.
struct Target<'a> {
    movie: &'a MovieDetails,
    truth: f32,
    credits: Vec<Credit>,
}

/// `films` with at least [`MIN_TRUTH_VOTES`] votes, sorted by TMDB id so any split
/// of them is the same every run.
fn targets<'a>(
    films: &'a [MovieDetails],
    weights: &FieldWeights,
    cast_limit: usize,
) -> Vec<Target<'a>> {
    let mut targets: Vec<Target> = films
        .iter()
        .filter(|m| m.vote_count.unwrap_or(0) >= MIN_TRUTH_VOTES)
        .filter_map(|movie| {
            Some(Target {
                movie,
                truth: movie.audience_rating()?,
                credits: credits(movie, weights, cast_limit),
            })
        })
        .collect();
    targets.sort_by_key(|t| t.movie.id);
    targets
}

/// Estimates every target from its credits in `roles` (all of them when `None`),
/// using the typical score where there's no estimate. Returns the share of targets
/// with an estimate, and how the estimates compare with the real scores.
fn score(
    targets: &[Target],
    filmographies: &HashMap<u64, Vec<CreditedFilm>>,
    typical: f32,
    settings: &EstimateSettings,
    roles: Option<&[Role]>,
) -> (f32, Scores) {
    let mut covered = 0;
    let (estimates, actual): (Vec<f32>, Vec<f32>) = targets
        .iter()
        .map(|target| {
            let used: Vec<Credit> = target
                .credits
                .iter()
                .filter(|c| roles.is_none_or(|roles| roles.contains(&c.role)))
                .cloned()
                .collect();
            let found = estimate_with(target.movie, &used, filmographies, typical, settings);
            covered += usize::from(found.is_some());
            (found.unwrap_or(typical), target.truth)
        })
        .unzip();
    let n = targets.len().max(1) as f32;
    let errors = estimates.iter().zip(&actual).map(|(e, a)| e - a);
    let scores = Scores {
        mae: errors.clone().map(f32::abs).sum::<f32>() / n,
        rmse: (errors.map(|e| e * e).sum::<f32>() / n).sqrt(),
        spearman: spearman(&estimates, &actual),
    };
    (covered as f32 / n, scores)
}
