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

/// The fit stops once its residual is this small relative to where it started:
/// far finer than the 0.1-point shifts the app shows.
const TOLERANCE: f64 = 1e-10;

/// A cap on the fit's iterations. It normally converges in a few hundred, since
/// the ridge keeps the system well conditioned.
const MAX_ITERATIONS: usize = 5_000;

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
    pub fn fit_with(rated: &[RatedDetails], weights: FieldWeights, ridge: f32) -> Option<Self> {
        let train: Vec<(Features, f64)> = rated
            .iter()
            .filter_map(|r| {
                let diff = r.rating - r.movie.audience_rating()?;
                Some((features::extract(&r.movie, &weights), f64::from(diff)))
            })
            .collect();
        let (average, per_feature) = fit_features(&train, f64::from(ridge))?;
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

/// The average of `train`'s differences, and one shift per feature fitted by ridge
/// regression to how far each movie's difference strays from it. `None` for no
/// movies.
///
/// Solves `(Xᵀ X + ridge × I) w = Xᵀ t` (movies' features `X`, centred differences
/// `t`) by conjugate gradient, touching only the features each movie has, so time
/// and memory grow with the number of ratings rather than its square or cube. The
/// same solution as the dual form [`solve_positive_definite`] finds from the
/// movies' pairwise overlaps, which a few thousand ratings would make seconds of
/// work and tens of megabytes.
fn fit_features(train: &[(Features, f64)], ridge: f64) -> Option<(f64, BTreeMap<String, f64>)> {
    if train.is_empty() {
        return None;
    }
    let average = train.iter().map(|(_, d)| d).sum::<f64>() / train.len() as f64;
    // Each distinct feature gets a column; each movie is a sparse row of them.
    let mut columns: BTreeMap<&str, usize> = BTreeMap::new();
    let rows: Vec<(Vec<(usize, f64)>, f64)> = train
        .iter()
        .map(|(feats, diff)| {
            let row = feats
                .iter()
                .map(|(token, value)| {
                    let next = columns.len();
                    (*columns.entry(token).or_insert(next), f64::from(*value))
                })
                .collect();
            (row, diff - average)
        })
        .collect();
    let width = columns.len();
    // v ↦ (Xᵀ X + ridge × I) v, one pass over the rows.
    let apply = |v: &[f64]| -> Vec<f64> {
        let mut out: Vec<f64> = v.iter().map(|x| ridge * x).collect();
        for (row, _) in &rows {
            let along: f64 = row.iter().map(|&(j, x)| x * v[j]).sum();
            for &(j, x) in row {
                out[j] += x * along;
            }
        }
        out
    };
    let mut target = vec![0.0; width];
    for (row, t) in &rows {
        for &(j, x) in row {
            target[j] += x * t;
        }
    }
    let shifts = conjugate_gradient(apply, &target);
    let per_feature = columns
        .into_iter()
        .map(|(token, j)| (token.to_string(), shifts[j]))
        .collect();
    Some((average, per_feature))
}

/// Solves `a x = b` for a symmetric positive-definite `a`, given only `x ↦ a x`,
/// starting from 0 and stopping at [`TOLERANCE`] or [`MAX_ITERATIONS`].
fn conjugate_gradient(apply: impl Fn(&[f64]) -> Vec<f64>, b: &[f64]) -> Vec<f64> {
    let dot = |x: &[f64], y: &[f64]| -> f64 { x.iter().zip(y).map(|(a, b)| a * b).sum() };
    let mut x = vec![0.0; b.len()];
    let mut residual = b.to_vec();
    let mut direction = residual.clone();
    let mut squared = dot(&residual, &residual);
    let stop = squared * TOLERANCE * TOLERANCE;
    for _ in 0..MAX_ITERATIONS {
        if squared <= stop || squared == 0.0 {
            break;
        }
        let along = apply(&direction);
        let step = squared / dot(&direction, &along);
        for ((xi, ri), (di, ai)) in x
            .iter_mut()
            .zip(residual.iter_mut())
            .zip(direction.iter().zip(&along))
        {
            *xi += step * di;
            *ri -= step * ai;
        }
        let next = dot(&residual, &residual);
        let keep = next / squared;
        for (di, ri) in direction.iter_mut().zip(&residual) {
            *di = ri + keep * *di;
        }
        squared = next;
    }
    x
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

#[cfg(test)]
mod tests {
    use super::*;

    fn features(pairs: &[(&str, f32)]) -> Features {
        pairs.iter().map(|(t, v)| (t.to_string(), *v)).collect()
    }

    /// The per-feature shifts from the dual form: `α = (K + ridge × I)⁻¹ t`, then
    /// each feature's shift is the sum over movies of α × its value.
    fn dual_shifts(train: &[(Features, f64)], ridge: f64) -> BTreeMap<String, f64> {
        let average = train.iter().map(|(_, d)| d).sum::<f64>() / train.len() as f64;
        let targets: Vec<f64> = train.iter().map(|(_, d)| d - average).collect();
        let system = train
            .iter()
            .enumerate()
            .map(|(a, (fa, _))| {
                train
                    .iter()
                    .enumerate()
                    .map(|(b, (fb, _))| dot(fa, fb) + if a == b { ridge } else { 0.0 })
                    .collect()
            })
            .collect();
        let alpha = solve_positive_definite(system, &targets).expect("positive definite");
        let mut shifts = BTreeMap::new();
        for ((feats, _), a) in train.iter().zip(&alpha) {
            for (token, value) in feats {
                *shifts.entry(token.clone()).or_insert(0.0) += a * f64::from(*value);
            }
        }
        shifts
    }

    #[test]
    fn conjugate_gradient_matches_the_exact_dual_solution() {
        // Overlapping genres, directors and actors, with differences that only
        // partly follow them, so no shift is trivially 0.
        let train: Vec<(Features, f64)> = (0..40)
            .map(|i: u32| {
                let feats = features(&[
                    (["g:drama", "g:horror", "g:comedy"][i as usize % 3], 2.0),
                    (&format!("dir:{}", i % 7), 3.0),
                    (&format!("cast:{}", i % 11), 2.0),
                    (&format!("cast:{}", (i * 5) % 13 + 20), 1.0),
                    (&format!("kw:{}", i % 5), 1.5),
                ]);
                let diff = f64::from(i % 7) * 0.3 - f64::from(i % 3) * 0.5 + f64::from(i % 4) * 0.1;
                (feats, diff)
            })
            .collect();
        for ridge in [1.0, 10.0, 100.0] {
            let (_, fitted) = fit_features(&train, ridge).expect("movies");
            let exact = dual_shifts(&train, ridge);
            assert_eq!(fitted.len(), exact.len());
            for (token, shift) in &exact {
                assert!(
                    (fitted[token] - shift).abs() < 1e-7,
                    "{token}: {} vs {shift} at ridge {ridge}",
                    fitted[token]
                );
            }
        }
    }

    #[test]
    fn identical_differences_need_no_shift() {
        let train = vec![
            (features(&[("g:drama", 2.0)]), 1.0),
            (features(&[("g:horror", 2.0)]), 1.0),
        ];
        let (average, shifts) = fit_features(&train, 10.0).expect("movies");
        assert_eq!(average, 1.0);
        assert!(shifts.values().all(|s| *s == 0.0), "{shifts:?}");
    }
}
