//! How a user's ratings differ from the TMDB audience ("diff" = their rating minus
//! the audience score), learned per feature: say they rate a director's films a point
//! above the crowd and horror half a point below. Pure.

use std::collections::BTreeMap;

use crate::features::{self, Features, FieldWeights};
use crate::profile::RatedDetails;
use crate::tmdb::MovieDetails;

/// How hard each feature's shift is pulled toward 0 when the evidence for it is
/// thin, in the units of a movie's squared field weights (one director alone
/// contributes 3² = 9). The "light" strength, which predicted best on MovieLens.
pub const RIDGE: f32 = 10.0;

/// How many features [`AudienceShift::predict`] names as reasons.
const MAX_REASONS: usize = 3;

/// A user's average difference from the audience plus a shift per feature (genre,
/// director, writer, actor, keyword), fitted jointly by ridge regression to how far
/// each rated movie's difference strays from that average. Fitting jointly means a
/// director who's also the writer isn't counted twice.
#[derive(Debug, Clone)]
pub struct AudienceShift {
    weights: FieldWeights,
    /// Mean of the user's differences from the audience.
    average: f32,
    /// Shift per feature token, per unit of that feature's field weight.
    per_feature: BTreeMap<String, f64>,
}

/// The difference [`AudienceShift`] predicts for one movie.
#[derive(Debug, Clone, PartialEq)]
pub struct ShiftPrediction {
    /// The user's average difference from the audience.
    pub average: f32,
    /// The movie's features' shifts on top of `average`, summed.
    pub features: f32,
    /// The features moving the prediction most, strongest first, as (label, shift),
    /// e.g. `("director Denis Villeneuve", 0.6)`.
    pub reasons: Vec<(String, f32)>,
}

impl ShiftPrediction {
    /// The predicted difference: `average + features`.
    pub fn total(&self) -> f32 {
        self.average + self.features
    }
}

impl AudienceShift {
    /// Fits the shift to the rated movies that have audience votes, at strength
    /// [`RIDGE`]. `None` when none have votes.
    pub fn fit(rated: &[RatedDetails], weights: FieldWeights) -> Option<Self> {
        Self::fit_with(rated, weights, RIDGE)
    }

    /// [`AudienceShift::fit`] at another ridge strength.
    ///
    /// Solved in its dual form, `(K + ridge × I) α = differences` with `K` the movies'
    /// pairwise feature overlaps, since there are far fewer movies than features.
    pub fn fit_with(rated: &[RatedDetails], weights: FieldWeights, ridge: f32) -> Option<Self> {
        let train: Vec<(Features, f64)> = rated
            .iter()
            .filter_map(|r| {
                let diff = r.rating - r.movie.audience_rating()?;
                Some((features::extract(&r.movie, &weights), f64::from(diff)))
            })
            .collect();
        if train.is_empty() {
            return None;
        }
        let average = train.iter().map(|(_, d)| d).sum::<f64>() / train.len() as f64;
        let targets: Vec<f64> = train.iter().map(|(_, d)| d - average).collect();
        let system: Vec<Vec<f64>> = train
            .iter()
            .enumerate()
            .map(|(a, (fa, _))| {
                train
                    .iter()
                    .enumerate()
                    .map(|(b, (fb, _))| {
                        let diagonal = if a == b { f64::from(ridge) } else { 0.0 };
                        dot(fa, fb) + diagonal
                    })
                    .collect()
            })
            .collect();
        let alpha = solve_positive_definite(system, &targets)?;
        // Back to one shift per feature: the sum over movies of α × feature value.
        let mut per_feature = BTreeMap::new();
        for ((feats, _), a) in train.iter().zip(&alpha) {
            for (token, value) in feats {
                *per_feature.entry(token.clone()).or_insert(0.0) += a * f64::from(*value);
            }
        }
        Some(Self {
            weights,
            average: average as f32,
            per_feature,
        })
    }

    /// The user's predicted difference from the audience for `movie`.
    pub fn predict(&self, movie: &MovieDetails) -> ShiftPrediction {
        let labels = features::labels(movie, &self.weights);
        let mut contributions: Vec<(String, f32)> = features::extract(movie, &self.weights)
            .iter()
            .filter_map(|(token, value)| {
                let shift = self.per_feature.get(token)? * f64::from(*value);
                Some((token.clone(), shift as f32))
            })
            .collect();
        let total = contributions.iter().map(|(_, s)| s).sum();
        contributions.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
        let reasons = contributions
            .into_iter()
            .filter(|(_, shift)| shift.abs() >= 0.05)
            .filter_map(|(token, shift)| Some((labels.get(&token)?.clone(), shift)))
            .take(MAX_REASONS)
            .collect();
        ShiftPrediction {
            average: self.average,
            features: total,
            reasons,
        }
    }
}

/// Sum of products of the features two movies share.
pub(crate) fn dot(a: &Features, b: &Features) -> f64 {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    small
        .iter()
        .filter_map(|(token, x)| large.get(token).map(|y| f64::from(x * y)))
        .sum()
}

/// Solves `a x = b` for a symmetric positive-definite `a` by Cholesky decomposition.
/// `None` if `a` isn't positive definite.
#[allow(clippy::needless_range_loop)] // index-based reads the clearest for this algebra
pub(crate) fn solve_positive_definite(mut a: Vec<Vec<f64>>, b: &[f64]) -> Option<Vec<f64>> {
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
