//! Leave-one-out evaluation: how well the scoring predicts the user's own ratings.
//!
//! Each rated movie is hidden in turn and predicted from all the others, then the
//! predictions are compared with the real ratings, next to simple baselines. Pure:
//! the caller supplies the rated movies' details and the rarity table.

use std::collections::HashMap;

use crate::features::{self, Features, FieldWeights};
use crate::profile::{RatedDetails, TasteProfile};
use crate::scoring::{self, NEIGHBOURS, OVERALL_SHRINK, SIMILARITY_POWER};
use crate::tfidf::{Idf, Vector};

/// The methods compared, in the order they're reported. From "audience + avg diff"
/// on, each predicts the audience score plus the user's usual difference from it
/// ("diff" = user rating minus TMDB audience score): on average across all their
/// ratings, on the movies most similar to this one, or as a per-feature shift
/// (one per genre, director, writer, actor and keyword) fitted by ridge regression
/// at each strength in [`SHIFT_RIDGE`].
///
/// The "est. quality" methods treat the hidden movie as unreleased: in place of its
/// real audience score they use an estimate made without it (from cast and crew
/// track records, see [`crate::quality`]). The user's diffs still come from their
/// other movies' real scores. They only predict when estimates are supplied.
pub const METHODS: [&str; 13] = [
    "movie-match",
    "your average",
    "closest rated movie",
    "TMDB audience score",
    "audience + avg diff",
    "audience + similar diff",
    "audience + shift (light)",
    "audience + shift (medium)",
    "audience + shift (strong)",
    "est. quality",
    "est. quality + avg diff",
    "est. quality + shift (light)",
    "est. quality + shift (medium)",
];

/// Ridge strengths for the per-feature shift, light to strong: how hard each
/// feature's shift is pulled toward 0 when the evidence for it is thin. In the units
/// of a movie's squared field weights (one director alone contributes 3² = 9).
pub const SHIFT_RIDGE: [f32; 3] = [10.0, 30.0, 100.0];

/// One rated movie's real rating and what each method predicted for it.
#[derive(Debug, Clone)]
pub struct EvaluatedMovie {
    pub title: String,
    pub rating: f32,
    /// One entry per [`METHODS`] item; `None` where a method has no prediction
    /// (the audience-based ones for a movie nobody has voted on).
    pub predictions: [Option<f32>; 13],
}

/// How one method did across all the movies it could predict.
#[derive(Debug, Clone)]
pub struct MethodResult {
    pub name: &'static str,
    /// Movies it made a prediction for.
    pub count: usize,
    /// Mean absolute error, in rating points (0 to 10).
    pub mae: f32,
    /// Root mean squared error: like `mae`, but big misses count more.
    pub rmse: f32,
    /// Spearman rank correlation with the real ratings, -1 to 1: how well the
    /// method orders the movies. `None` for a method that guesses (nearly) the same
    /// number for every movie, which can't order anything.
    pub spearman: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct Evaluation {
    pub movies: Vec<EvaluatedMovie>,
    /// One per [`METHODS`] item, in that order.
    pub results: Vec<MethodResult>,
}

/// Predicts each of `rated` from all the others with every method in [`METHODS`].
/// Needs at least two rated movies to predict anything. No quality estimates, so
/// the "est. quality" methods predict nothing; see [`leave_one_out_with_quality`].
pub fn leave_one_out(rated: &[RatedDetails], idf: &Idf, weights: FieldWeights) -> Evaluation {
    leave_one_out_with_quality(rated, idf, weights, None)
}

/// [`leave_one_out`], with `quality` holding a pre-release quality estimate per TMDB
/// id for the "est. quality" methods.
pub fn leave_one_out_with_quality(
    rated: &[RatedDetails],
    idf: &Idf,
    weights: FieldWeights,
    quality: Option<&HashMap<u64, f32>>,
) -> Evaluation {
    let mut movies = Vec::with_capacity(rated.len());
    if rated.len() >= 2 {
        // Field-weighted features (no rarity weighting) and every pair's overlap,
        // for the per-feature shift.
        let feats: Vec<Features> = rated
            .iter()
            .map(|r| features::extract(&r.movie, &weights))
            .collect();
        let overlap: Vec<Vec<f64>> = feats
            .iter()
            .map(|a| feats.iter().map(|b| dot(a, b)).collect())
            .collect();
        for (i, hidden) in rated.iter().enumerate() {
            let others: Vec<RatedDetails> = rated
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, r)| r.clone())
                .collect();
            let profile = TasteProfile::new(&others, idf.clone(), weights);
            let candidate = profile.vectorize(&hidden.movie);
            let prediction = scoring::predict(&profile, &candidate);
            let average = profile.mean_rating();
            let closest = prediction
                .neighbours
                .first()
                .map_or(average, |n| n.rated.rating);
            let audience = hidden.movie.audience_rating();
            let (avg_diff, similar_diff) = audience_differences(&others, &profile, &candidate);
            let plus = |diff: Option<f32>| Some((audience? + diff?).clamp(0.0, 10.0));
            let estimated = quality.and_then(|q| q.get(&hidden.movie.id)).copied();
            let plus_estimated = |diff: Option<f32>| Some((estimated? + diff?).clamp(0.0, 10.0));
            let [light, medium, strong] = feature_shifts(rated, &overlap, i);
            movies.push(EvaluatedMovie {
                title: hidden.movie.title.clone(),
                rating: hidden.rating,
                predictions: [
                    Some(prediction.rating),
                    Some(average),
                    Some(closest),
                    audience,
                    plus(avg_diff),
                    plus(similar_diff),
                    plus(light),
                    plus(medium),
                    plus(strong),
                    estimated,
                    plus_estimated(avg_diff),
                    plus_estimated(light),
                    plus_estimated(medium),
                ],
            });
        }
    }
    let results = METHODS
        .iter()
        .enumerate()
        .map(|(m, &name)| {
            let pairs: Vec<(f32, f32)> = movies
                .iter()
                .filter_map(|movie| Some((movie.predictions[m]?, movie.rating)))
                .collect();
            // Leaving one movie out shifts the average a little each time, which
            // would rank movies exactly backwards; it's a constant guess at heart.
            let ranks = name != "your average";
            summarize(name, &pairs, ranks)
        })
        .collect();
    Evaluation { movies, results }
}

/// The user's difference from the TMDB audience (their rating minus the audience
/// score), as `(average over all of `others`, estimate for this candidate)`.
///
/// The candidate's estimate uses the same neighbour rules as [`scoring::predict`]:
/// the [`NEIGHBOURS`] most similar movies, weighted by (similarity / closest) ^
/// [`SIMILARITY_POWER`], blended with the average difference by count as
/// `(n × weighted + OVERALL_SHRINK × average) / (n + OVERALL_SHRINK)`. Movies without
/// audience votes are skipped. `None` when none of `others` has votes.
fn audience_differences(
    others: &[RatedDetails],
    profile: &TasteProfile,
    candidate: &Vector,
) -> (Option<f32>, Option<f32>) {
    // `profile.rated()` holds `others` in the same order.
    let mut diffs: Vec<(f32, f32)> = others
        .iter()
        .zip(profile.rated())
        .filter_map(|(other, vector)| {
            let diff = other.rating - other.movie.audience_rating()?;
            Some((candidate.cosine(&vector.vector), diff))
        })
        .collect();
    if diffs.is_empty() {
        return (None, None);
    }
    let average = diffs.iter().map(|(_, d)| d).sum::<f32>() / diffs.len() as f32;

    diffs.retain(|(similarity, _)| *similarity > 0.0);
    // Stable sort, like scoring: ties keep rating-file order.
    diffs.sort_by(|a, b| b.0.total_cmp(&a.0));
    diffs.truncate(NEIGHBOURS);
    let top = diffs.first().map_or(0.0, |(s, _)| *s);
    let (mut total, mut weighted) = (0.0, 0.0);
    for (similarity, diff) in &diffs {
        let weight = (similarity / top).powi(SIMILARITY_POWER);
        total += weight;
        weighted += weight * diff;
    }
    let n = diffs.len() as f32;
    let similar = if total > 0.0 {
        (n * (weighted / total) + OVERALL_SHRINK * average) / (n + OVERALL_SHRINK)
    } else {
        average
    };
    (Some(average), Some(similar))
}

/// The user's predicted difference from the audience for `rated[hidden]`, once per
/// [`SHIFT_RIDGE`] strength: their average difference over the other movies, plus a
/// shift per feature (genre, director, writer, actor, keyword) fitted jointly by
/// ridge regression to how far each other movie's difference strays from that
/// average. Fitting the shifts jointly means a director who's also the writer isn't
/// counted twice; the ridge pulls shifts with little evidence toward 0.
///
/// Solved in its dual form, `(K + ridge × I) α = differences` with `K` the movies'
/// pairwise feature overlaps, since there are far fewer movies than features.
/// `overlap[a][b]` is that overlap for every pair in `rated`. Movies without
/// audience votes are skipped; `None` when none of the others has votes.
fn feature_shifts(rated: &[RatedDetails], overlap: &[Vec<f64>], hidden: usize) -> [Option<f32>; 3] {
    let train: Vec<(usize, f64)> = rated
        .iter()
        .enumerate()
        .filter(|(j, _)| *j != hidden)
        .filter_map(|(j, r)| Some((j, f64::from(r.rating - r.movie.audience_rating()?))))
        .collect();
    if train.is_empty() {
        return [None; 3];
    }
    let average = train.iter().map(|(_, d)| d).sum::<f64>() / train.len() as f64;
    let targets: Vec<f64> = train.iter().map(|(_, d)| d - average).collect();
    SHIFT_RIDGE.map(|ridge| {
        let system: Vec<Vec<f64>> = train
            .iter()
            .enumerate()
            .map(|(a, (ja, _))| {
                train
                    .iter()
                    .enumerate()
                    .map(|(b, (jb, _))| {
                        let diagonal = if a == b { f64::from(ridge) } else { 0.0 };
                        overlap[*ja][*jb] + diagonal
                    })
                    .collect()
            })
            .collect();
        let alpha = solve_positive_definite(system, &targets)?;
        let shift: f64 = train
            .iter()
            .zip(&alpha)
            .map(|((j, _), a)| a * overlap[hidden][*j])
            .sum();
        Some((average + shift) as f32)
    })
}

/// Sum of products of the features two movies share.
fn dot(a: &Features, b: &Features) -> f64 {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    small
        .iter()
        .filter_map(|(token, x)| large.get(token).map(|y| f64::from(x * y)))
        .sum()
}

/// Solves `a x = b` for a symmetric positive-definite `a` by Cholesky decomposition.
/// `None` if `a` isn't positive definite.
#[allow(clippy::needless_range_loop)] // index-based reads the clearest for this algebra
fn solve_positive_definite(mut a: Vec<Vec<f64>>, b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    // In place: the lower triangle of `a` becomes L, with a = L × Lᵀ.
    for j in 0..n {
        let mut d = a[j][j];
        for k in 0..j {
            d -= a[j][k] * a[j][k];
        }
        if d <= 0.0 {
            return None;
        }
        let d = d.sqrt();
        a[j][j] = d;
        for i in j + 1..n {
            let mut s = a[i][j];
            for k in 0..j {
                s -= a[i][k] * a[j][k];
            }
            a[i][j] = s / d;
        }
    }
    // L y = b, then Lᵀ x = y.
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i][k] * y[k];
        }
        y[i] = s / a[i][i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = y[i];
        for k in i + 1..n {
            s -= a[k][i] * x[k];
        }
        x[i] = s / a[i][i];
    }
    Some(x)
}

fn summarize(name: &'static str, pairs: &[(f32, f32)], ranks: bool) -> MethodResult {
    let count = pairs.len();
    if count == 0 {
        return MethodResult {
            name,
            count,
            mae: 0.0,
            rmse: 0.0,
            spearman: None,
        };
    }
    let n = count as f32;
    let mae = pairs.iter().map(|(p, r)| (p - r).abs()).sum::<f32>() / n;
    let rmse = (pairs.iter().map(|(p, r)| (p - r).powi(2)).sum::<f32>() / n).sqrt();
    let (predicted, actual): (Vec<f32>, Vec<f32>) = pairs.iter().copied().unzip();
    MethodResult {
        name,
        count,
        mae,
        rmse,
        spearman: if ranks {
            spearman(&predicted, &actual)
        } else {
            None
        },
    }
}

/// [`leave_one_out_with_quality`] for each user's ratings, spread over the CPU's threads. Results
/// come back in the same order as `users`.
pub fn leave_one_out_each(
    users: &[Vec<RatedDetails>],
    idf: &Idf,
    weights: FieldWeights,
    quality: Option<&HashMap<u64, f32>>,
) -> Vec<Evaluation> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = users.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = users
            .chunks(chunk)
            .map(|group| {
                scope.spawn(move || {
                    group
                        .iter()
                        .map(|rated| leave_one_out_with_quality(rated, idf, weights, quality))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            // A panicking worker would otherwise silently drop its users.
            .flat_map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|p| std::panic::resume_unwind(p))
            })
            .collect()
    })
}

/// How the methods did across many users' leave-one-out evaluations.
#[derive(Debug, Clone)]
pub struct UsersReport {
    /// Users with at least one prediction.
    pub users: usize,
    /// Every user's predictions pooled, one per [`METHODS`] item. `spearman` is
    /// always `None` here: ranks only mean something within one user's ratings.
    pub pooled: Vec<MethodResult>,
    /// The mean of each user's own Spearman correlation, per method.
    pub mean_spearman: Vec<Option<f32>>,
    /// Share of users (0 to 1) for whom each method had the lowest MAE.
    pub wins: Vec<f32>,
    /// Users in four equal groups by how closely their ratings follow the TMDB
    /// audience (Spearman of their ratings against audience scores), most like the
    /// crowd first. Users with too few audience scores to tell are left out.
    pub taste_groups: Vec<TasteGroup>,
}

#[derive(Debug, Clone)]
pub struct TasteGroup {
    pub users: usize,
    /// The lowest and highest agreement with the audience in the group, -1 to 1.
    pub agreement: (f32, f32),
    /// Each method's MAE over the group's pooled predictions, per [`METHODS`] item.
    pub mae: Vec<f32>,
}

/// Combines per-user evaluations (from [`leave_one_out_each`]) into one report.
pub fn across_users(evaluations: &[Evaluation]) -> UsersReport {
    let evaluations: Vec<&Evaluation> = evaluations
        .iter()
        .filter(|e| !e.movies.is_empty())
        .collect();
    let pooled_mae = |group: &[&Evaluation]| -> Vec<MethodResult> {
        METHODS
            .iter()
            .enumerate()
            .map(|(m, &name)| {
                let pairs: Vec<(f32, f32)> = group
                    .iter()
                    .flat_map(|e| &e.movies)
                    .filter_map(|movie| Some((movie.predictions[m]?, movie.rating)))
                    .collect();
                summarize(name, &pairs, false)
            })
            .collect()
    };

    let mean_spearman = (0..METHODS.len())
        .map(|m| {
            let values: Vec<f32> = evaluations
                .iter()
                .filter_map(|e| e.results[m].spearman)
                .collect();
            (!values.is_empty()).then(|| values.iter().sum::<f32>() / values.len() as f32)
        })
        .collect();

    let mut wins = vec![0usize; METHODS.len()];
    for evaluation in &evaluations {
        let best = evaluation
            .results
            .iter()
            .enumerate()
            .filter(|(_, r)| r.count > 0)
            .min_by(|a, b| a.1.mae.total_cmp(&b.1.mae));
        if let Some((m, _)) = best {
            wins[m] += 1;
        }
    }
    let users = evaluations.len();
    let wins = wins
        .into_iter()
        .map(|w| {
            if users == 0 {
                0.0
            } else {
                w as f32 / users as f32
            }
        })
        .collect();

    // Agreement with the crowd: the user's ratings against the audience scores.
    let mut by_agreement: Vec<(f32, &Evaluation)> = evaluations
        .iter()
        .filter_map(|e| {
            let (ratings, audience): (Vec<f32>, Vec<f32>) = e
                .movies
                .iter()
                .filter_map(|movie| Some((movie.rating, movie.predictions[3]?)))
                .unzip();
            Some((spearman(&ratings, &audience)?, *e))
        })
        .collect();
    by_agreement.sort_by(|a, b| b.0.total_cmp(&a.0));
    let group_size = by_agreement.len().div_ceil(4).max(1);
    let taste_groups = by_agreement
        .chunks(group_size)
        .map(|group| {
            let members: Vec<&Evaluation> = group.iter().map(|(_, e)| *e).collect();
            TasteGroup {
                users: group.len(),
                agreement: (group[group.len() - 1].0, group[0].0),
                mae: pooled_mae(&members).iter().map(|r| r.mae).collect(),
            }
        })
        .collect();

    UsersReport {
        users,
        pooled: pooled_mae(&evaluations),
        mean_spearman,
        wins,
        taste_groups,
    }
}

/// Strengths tried for the track-record nudge on top of movie-match's score.
pub const NUDGE_STRENGTHS: [f32; 8] = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.75, 1.0];

/// How a set of predictions did across several users.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupScores {
    /// Over every prediction pooled.
    pub mae: f32,
    pub rmse: f32,
    /// The mean of each user's own Spearman correlation.
    pub mean_spearman: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct NudgeReport {
    pub tune_users: usize,
    pub check_users: usize,
    /// The strength with the lowest MAE on the tuning users.
    pub best: f32,
    /// Every strength in [`NUDGE_STRENGTHS`] on the checking users, which tuning
    /// never saw. Strength 0 is movie-match unchanged.
    pub check: Vec<(f32, GroupScores)>,
}

/// Tunes `movie-match + strength × (estimated quality − typical)` (clamped to 0 to
/// 10): picks the [`NUDGE_STRENGTHS`] value with the lowest MAE on every other user
/// in `evaluations`, then scores every strength on the remaining users. Uses the
/// "movie-match" and "est. quality" predictions, so the evaluations need quality
/// estimates; `None` without any. Ties keep the weaker nudge.
pub fn tune_nudge(evaluations: &[Evaluation], typical: f32) -> Option<NudgeReport> {
    let movie_match = METHODS.iter().position(|&m| m == "movie-match")?;
    let estimated = METHODS.iter().position(|&m| m == "est. quality")?;
    // Per user: (real rating, movie-match prediction, estimated quality).
    let users: Vec<Vec<(f32, f32, f32)>> = evaluations
        .iter()
        .map(|e| {
            e.movies
                .iter()
                .filter_map(|m| {
                    Some((
                        m.rating,
                        m.predictions[movie_match]?,
                        m.predictions[estimated]?,
                    ))
                })
                .collect::<Vec<_>>()
        })
        .filter(|user| !user.is_empty())
        .collect();
    if users.is_empty() {
        return None;
    }
    let (tune, check): (Vec<_>, Vec<_>) =
        users.into_iter().enumerate().partition(|(i, _)| i % 2 == 0);
    let tune: Vec<_> = tune.into_iter().map(|(_, u)| u).collect();
    let check: Vec<_> = check.into_iter().map(|(_, u)| u).collect();

    let scores = |users: &[Vec<(f32, f32, f32)>], strength: f32| {
        let nudged = |&(_, base, quality): &(f32, f32, f32)| {
            (base + strength * (quality - typical)).clamp(0.0, 10.0)
        };
        let errors: Vec<f32> = users.iter().flatten().map(|p| nudged(p) - p.0).collect();
        let n = errors.len().max(1) as f32;
        let correlations: Vec<f32> = users
            .iter()
            .filter_map(|user| {
                let predicted: Vec<f32> = user.iter().map(nudged).collect();
                let actual: Vec<f32> = user.iter().map(|p| p.0).collect();
                spearman(&predicted, &actual)
            })
            .collect();
        GroupScores {
            mae: errors.iter().map(|e| e.abs()).sum::<f32>() / n,
            rmse: (errors.iter().map(|e| e * e).sum::<f32>() / n).sqrt(),
            mean_spearman: (!correlations.is_empty())
                .then(|| correlations.iter().sum::<f32>() / correlations.len() as f32),
        }
    };

    let mut best = (NUDGE_STRENGTHS[0], scores(&tune, NUDGE_STRENGTHS[0]).mae);
    for strength in NUDGE_STRENGTHS {
        let mae = scores(&tune, strength).mae;
        if mae < best.1 {
            best = (strength, mae);
        }
    }
    Some(NudgeReport {
        tune_users: tune.len(),
        check_users: check.len(),
        best: best.0,
        check: NUDGE_STRENGTHS
            .iter()
            .map(|&strength| (strength, scores(&check, strength)))
            .collect(),
    })
}

/// Spearman rank correlation of two equally long lists, -1 to 1, with tied values
/// sharing their average rank. `None` when either list has no spread (all equal)
/// or there are fewer than two values.
pub fn spearman(xs: &[f32], ys: &[f32]) -> Option<f32> {
    if xs.len() != ys.len() || xs.len() < 2 {
        return None;
    }
    pearson(&ranks(xs), &ranks(ys))
}

/// 1-based ranks, ties getting the average of the ranks they span.
fn ranks(values: &[f32]) -> Vec<f32> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
    let mut ranks = vec![0.0; values.len()];
    let mut start = 0;
    while start < order.len() {
        let mut end = start;
        while end + 1 < order.len() && values[order[end + 1]] == values[order[start]] {
            end += 1;
        }
        // Positions start..=end hold equal values; each gets their mean 1-based rank.
        let rank = (start + end) as f32 / 2.0 + 1.0;
        for &index in &order[start..=end] {
            ranks[index] = rank;
        }
        start = end + 1;
    }
    ranks
}

fn pearson(xs: &[f32], ys: &[f32]) -> Option<f32> {
    let n = xs.len() as f32;
    let mean_x = xs.iter().sum::<f32>() / n;
    let mean_y = ys.iter().sum::<f32>() / n;
    let (mut cov, mut var_x, mut var_y) = (0.0, 0.0, 0.0);
    for (x, y) in xs.iter().zip(ys) {
        cov += (x - mean_x) * (y - mean_y);
        var_x += (x - mean_x).powi(2);
        var_y += (y - mean_y).powi(2);
    }
    if var_x <= f32::EPSILON || var_y <= f32::EPSILON {
        return None;
    }
    Some((cov / (var_x.sqrt() * var_y.sqrt())).clamp(-1.0, 1.0))
}
