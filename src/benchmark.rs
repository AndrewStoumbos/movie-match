//! Benchmark: Movie Match against standard collaborative-filtering recommenders on
//! MovieLens.
//!
//! Each user's newest ratings are hidden (a time split: predict the films they'll
//! watch next), every method learns from the older ones and predicts the hidden ones.
//! The collaborative methods learn from every user's older ratings; Movie Match from
//! that user's alone, as in the app. Pure: the caller supplies the ratings, the
//! movies' details and the rarity table.

use std::collections::{BTreeMap, HashMap};

use crate::evaluate::{self, MethodResult};
use crate::features::FieldWeights;
use crate::movielens::UserRatings;
use crate::profile::{RatedDetails, TasteProfile};
use crate::scoring::{self, Signals, TrackRecords};
use crate::shift::AudienceShift;
use crate::tfidf::Idf;
use crate::tmdb::MovieDetails;

/// Share of each user's ratings hidden, newest first.
pub const TEST_SHARE: f32 = 0.2;
/// Users with fewer older ratings than this left after the split hide nothing; their
/// ratings still train the collaborative methods.
pub const MIN_TRAIN: usize = 5;

/// The methods compared, in the order they're reported.
///
/// - "your average": the user's mean training rating, the floor.
/// - "baseline": overall mean + user bias + film bias, fitted by alternating least
///   squares as in the Surprise library's `BaselineOnly`.
/// - "item k-NN": the baseline plus the user's leftovers on the 40 films rated most
///   alike by the same users (shrunk Pearson-baseline similarity, as `KNNBaseline`).
/// - "SVD": biased matrix factorisation by gradient descent (Funk SVD), with
///   Surprise's default settings.
/// - "similarity only": Movie Match's similarity rating alone.
/// - "movie-match, no audience": the unreleased formula (similarity + track record),
///   which never sees a TMDB audience score and so can't leak later votes.
/// - "movie-match": the full blend the app uses.
pub const METHODS: [&str; 7] = [
    "your average",
    "baseline",
    "item k-NN",
    "SVD",
    "similarity only",
    "movie-match, no audience",
    "movie-match",
];

/// Popularity buckets by how many training ratings a film has: lower bound, label.
pub const POPULARITY: [(usize, &str); 5] = [
    (0, "0"),
    (1, "1-4"),
    (5, "5-19"),
    (20, "20-99"),
    (100, "100+"),
];

/// Ranking quality is measured on each user's top this many hidden films.
pub const NDCG_AT: usize = 10;

/// Each user's ratings split by time, both newest first like [`UserRatings`].
#[derive(Debug, Clone, Default)]
pub struct Split {
    pub train: UserRatings,
    /// Only users with something hidden.
    pub test: UserRatings,
}

/// Hides each user's newest `test_share` of ratings (at least one), keeping the rest
/// for training. A user left with fewer than `min_train` training ratings hides
/// nothing.
pub fn split(users: &UserRatings, test_share: f32, min_train: usize) -> Split {
    let mut result = Split::default();
    for (&user, ratings) in users {
        let hidden = ((ratings.len() as f32 * test_share).ceil() as usize).max(1);
        if ratings.len() >= hidden + min_train {
            result.test.insert(user, ratings[..hidden].to_vec());
            result.train.insert(user, ratings[hidden..].to_vec());
        } else {
            result.train.insert(user, ratings.clone());
        }
    }
    result
}

/// Lowest and highest rating on the 1 to 10 scale (0.5 to 5 stars, doubled).
const RATING_RANGE: (f32, f32) = (1.0, 10.0);

const BASELINE_EPOCHS: usize = 10;
const BASELINE_REG_USER: f32 = 15.0;
const BASELINE_REG_ITEM: f32 = 10.0;
const KNN_NEIGHBOURS: usize = 40;
/// Similarities from few shared raters are pulled toward 0:
/// `× (n − 1) / (n − 1 + KNN_SHRINK)` for `n` shared raters.
const KNN_SHRINK: f32 = 100.0;
const SVD_FACTORS: usize = 100;
const SVD_EPOCHS: usize = 20;
const SVD_LEARNING_RATE: f32 = 0.005;
const SVD_REG: f32 = 0.02;
const SVD_INIT_STD: f64 = 0.1;

/// The collaborative methods, fitted once on every user's training ratings.
pub struct Collaborative {
    users: HashMap<u32, usize>,
    items: HashMap<u64, usize>,
    /// Per user: (item, rating), in training order.
    by_user: Vec<Vec<(usize, f32)>>,
    /// Per item: (user, rating), by user index.
    by_item: Vec<Vec<(usize, f32)>>,
    mean: f32,
    user_bias: Vec<f32>,
    item_bias: Vec<f32>,
    svd: Svd,
}

/// Funk SVD's learned parameters, on the 0.5 to 5 star scale its defaults are for.
struct Svd {
    mean: f32,
    user_bias: Vec<f32>,
    item_bias: Vec<f32>,
    user_factors: Vec<Vec<f32>>,
    item_factors: Vec<Vec<f32>>,
}

impl Collaborative {
    /// Fits every method on `train`. `seed` sets SVD's random starting factors, so the
    /// same seed always gives the same predictions.
    pub fn fit(train: &UserRatings, seed: u64) -> Self {
        let mut users = HashMap::new();
        let mut items = HashMap::new();
        let mut by_user = Vec::new();
        let mut by_item: Vec<Vec<(usize, f32)>> = Vec::new();
        let (mut sum, mut count) = (0.0, 0usize);
        for (&user, ratings) in train {
            let u = by_user.len();
            users.insert(user, u);
            let mut rated = Vec::with_capacity(ratings.len());
            for &(movie, rating) in ratings {
                let i = *items.entry(movie).or_insert_with(|| {
                    by_item.push(Vec::new());
                    by_item.len() - 1
                });
                rated.push((i, rating));
                by_item[i].push((u, rating));
                sum += rating;
                count += 1;
            }
            by_user.push(rated);
        }
        let mean = if count == 0 { 0.0 } else { sum / count as f32 };
        let (user_bias, item_bias) = fit_baseline(&by_user, &by_item, mean);
        let svd = fit_svd(&by_user, by_item.len(), seed);
        Self {
            users,
            items,
            by_user,
            by_item,
            mean,
            user_bias,
            item_bias,
            svd,
        }
    }

    /// How many training ratings `movie` has.
    pub fn ratings_of(&self, movie: u64) -> usize {
        self.items.get(&movie).map_or(0, |&i| self.by_item[i].len())
    }

    /// The baseline prediction: overall mean plus whichever biases are known.
    pub fn baseline(&self, user: u32, movie: u64) -> f32 {
        let u = self.users.get(&user).copied();
        let i = self.items.get(&movie).copied();
        clamp(self.biased(u, i))
    }

    fn biased(&self, u: Option<usize>, i: Option<usize>) -> f32 {
        self.mean + u.map_or(0.0, |u| self.user_bias[u]) + i.map_or(0.0, |i| self.item_bias[i])
    }

    /// The baseline plus the user's weighted leftovers on the [`KNN_NEIGHBOURS`] films
    /// most similar to `movie` among those they rated; the baseline alone when there
    /// are none (e.g. a film nobody rated).
    pub fn item_knn(&self, user: u32, movie: u64) -> f32 {
        let u = self.users.get(&user).copied();
        let i = self.items.get(&movie).copied();
        let baseline = self.biased(u, i);
        let (Some(u), Some(i)) = (u, i) else {
            return clamp(baseline);
        };
        let mut neighbours: Vec<(f32, f32)> = self.by_user[u]
            .iter()
            .filter(|(j, _)| *j != i)
            .filter_map(|&(j, rating)| {
                let similarity = self.similarity(i, j);
                let leftover = rating - self.biased(Some(u), Some(j));
                (similarity > 0.0).then_some((similarity, leftover))
            })
            .collect();
        neighbours.sort_by(|a, b| b.0.total_cmp(&a.0));
        neighbours.truncate(KNN_NEIGHBOURS);
        let total: f32 = neighbours.iter().map(|(s, _)| s).sum();
        if total <= 0.0 {
            return clamp(baseline);
        }
        let weighted: f32 = neighbours.iter().map(|(s, l)| s * l).sum();
        clamp(baseline + weighted / total)
    }

    /// Pearson-baseline similarity of two films over the users who rated both, shrunk
    /// toward 0 when they're few.
    fn similarity(&self, a: usize, b: usize) -> f32 {
        let (xs, ys) = (&self.by_item[a], &self.by_item[b]);
        let (mut x, mut y) = (0, 0);
        let (mut xy, mut xx, mut yy, mut shared) = (0.0f32, 0.0f32, 0.0f32, 0usize);
        // Both lists are in user order, so one pass finds the shared raters.
        while x < xs.len() && y < ys.len() {
            let (ux, rx) = xs[x];
            let (uy, ry) = ys[y];
            match ux.cmp(&uy) {
                std::cmp::Ordering::Less => x += 1,
                std::cmp::Ordering::Greater => y += 1,
                std::cmp::Ordering::Equal => {
                    let lx = rx - self.biased(Some(ux), Some(a));
                    let ly = ry - self.biased(Some(uy), Some(b));
                    xy += lx * ly;
                    xx += lx * lx;
                    yy += ly * ly;
                    shared += 1;
                    x += 1;
                    y += 1;
                }
            }
        }
        if shared < 2 || xx <= 0.0 || yy <= 0.0 {
            return 0.0;
        }
        let pearson = xy / (xx.sqrt() * yy.sqrt());
        let n = (shared - 1) as f32;
        pearson * n / (n + KNN_SHRINK)
    }

    /// SVD's prediction: the mean plus whichever biases and factors are known.
    pub fn svd(&self, user: u32, movie: u64) -> f32 {
        let u = self.users.get(&user).copied();
        let i = self.items.get(&movie).copied();
        let svd = &self.svd;
        let mut stars =
            svd.mean + u.map_or(0.0, |u| svd.user_bias[u]) + i.map_or(0.0, |i| svd.item_bias[i]);
        if let (Some(u), Some(i)) = (u, i) {
            stars += dot(&svd.user_factors[u], &svd.item_factors[i]);
        }
        clamp(stars * 2.0)
    }
}

/// User and item biases by alternating least squares: each pass sets every item's
/// bias from its ratings' leftovers, then every user's.
fn fit_baseline(
    by_user: &[Vec<(usize, f32)>],
    by_item: &[Vec<(usize, f32)>],
    mean: f32,
) -> (Vec<f32>, Vec<f32>) {
    let mut user_bias = vec![0.0; by_user.len()];
    let mut item_bias = vec![0.0; by_item.len()];
    for _ in 0..BASELINE_EPOCHS {
        for (i, ratings) in by_item.iter().enumerate() {
            let sum: f32 = ratings.iter().map(|&(u, r)| r - mean - user_bias[u]).sum();
            item_bias[i] = sum / (BASELINE_REG_ITEM + ratings.len() as f32);
        }
        for (u, ratings) in by_user.iter().enumerate() {
            let sum: f32 = ratings.iter().map(|&(i, r)| r - mean - item_bias[i]).sum();
            user_bias[u] = sum / (BASELINE_REG_USER + ratings.len() as f32);
        }
    }
    (user_bias, item_bias)
}

/// Biased matrix factorisation by stochastic gradient descent, visiting ratings user
/// by user like Surprise's `SVD`. Trained on stars (rating / 2), since its learning
/// rate and regularisation are tuned for that scale.
fn fit_svd(by_user: &[Vec<(usize, f32)>], items: usize, seed: u64) -> Svd {
    let mut rng = Rng(seed);
    let mut factors = |n: usize| -> Vec<Vec<f32>> {
        (0..n)
            .map(|_| {
                (0..SVD_FACTORS)
                    .map(|_| (rng.normal() * SVD_INIT_STD) as f32)
                    .collect()
            })
            .collect()
    };
    let mut user_factors = factors(by_user.len());
    let mut item_factors = factors(items);
    let mut user_bias = vec![0.0; by_user.len()];
    let mut item_bias = vec![0.0; items];
    let count: usize = by_user.iter().map(Vec::len).sum();
    let mean = if count == 0 {
        0.0
    } else {
        by_user.iter().flatten().map(|(_, r)| r / 2.0).sum::<f32>() / count as f32
    };
    let (rate, reg) = (SVD_LEARNING_RATE, SVD_REG);
    for _ in 0..SVD_EPOCHS {
        for (u, ratings) in by_user.iter().enumerate() {
            for &(i, rating) in ratings {
                let predicted =
                    mean + user_bias[u] + item_bias[i] + dot(&user_factors[u], &item_factors[i]);
                let err = rating / 2.0 - predicted;
                user_bias[u] += rate * (err - reg * user_bias[u]);
                item_bias[i] += rate * (err - reg * item_bias[i]);
                for f in 0..SVD_FACTORS {
                    let (p, q) = (user_factors[u][f], item_factors[i][f]);
                    user_factors[u][f] += rate * (err * q - reg * p);
                    item_factors[i][f] += rate * (err * p - reg * q);
                }
            }
        }
    }
    Svd {
        mean,
        user_bias,
        item_bias,
        user_factors,
        item_factors,
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn clamp(rating: f32) -> f32 {
    rating.clamp(RATING_RANGE.0, RATING_RANGE.1)
}

/// A small seeded random number generator (SplitMix64), so SVD's starting factors are
/// the same on every run without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1).
    fn uniform(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Standard normal, by the Box-Muller transform.
    fn normal(&mut self) -> f64 {
        let (a, b) = (self.uniform(), self.uniform());
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

/// One hidden rating and every method's prediction for it.
#[derive(Debug, Clone)]
pub struct Predicted {
    pub movie: u64,
    pub rating: f32,
    /// The film's ratings in the training data.
    pub popularity: usize,
    /// One per [`METHODS`] item.
    pub predictions: [f32; 7],
}

/// How every method did on the hidden ratings.
#[derive(Debug, Clone)]
pub struct BenchmarkReport {
    /// Users with at least one hidden rating predicted.
    pub users: usize,
    pub train_ratings: usize,
    /// Hidden ratings predicted (those TMDB has details for).
    pub predicted: usize,
    /// Hidden ratings left out for every method because TMDB has no details.
    pub skipped: usize,
    /// Every prediction pooled, one per [`METHODS`] item (`spearman` is `None`).
    pub overall: Vec<MethodResult>,
    /// The mean of each user's own Spearman correlation, per method.
    pub mean_spearman: Vec<Option<f32>>,
    /// The mean of each user's NDCG@[`NDCG_AT`] over their hidden films, per method:
    /// 1 when the films they rated highest are ranked first.
    pub mean_ndcg: Vec<Option<f32>>,
    /// MAE by how many training ratings the film has (see [`POPULARITY`]).
    pub by_popularity: Vec<PopularityBucket>,
}

#[derive(Debug, Clone)]
pub struct PopularityBucket {
    pub label: &'static str,
    pub ratings: usize,
    /// Per [`METHODS`] item; 0 when the bucket is empty.
    pub mae: Vec<f32>,
}

/// Predicts every hidden rating in `split` with every method. Movie Match uses each
/// user's training ratings that `details` has, with `track_records` for its nudge
/// when given; `idf` should come from the movies' features alone (no ratings).
pub fn run(
    split: &Split,
    details: &BTreeMap<u64, MovieDetails>,
    idf: &Idf,
    weights: FieldWeights,
    track_records: Option<&TrackRecords>,
    seed: u64,
) -> BenchmarkReport {
    let collaborative = Collaborative::fit(&split.train, seed);
    let users: Vec<(u32, &[(u64, f32)])> = split
        .test
        .iter()
        .map(|(&user, hidden)| (user, hidden.as_slice()))
        .collect();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = users.len().div_ceil(threads).max(1);
    let per_user: Vec<Vec<Predicted>> = std::thread::scope(|scope| {
        let handles: Vec<_> = users
            .chunks(chunk)
            .map(|group| {
                let collaborative = &collaborative;
                scope.spawn(move || {
                    let mut signals = Signals {
                        shift: None,
                        track_records: track_records.cloned(),
                    };
                    group
                        .iter()
                        .map(|&(user, hidden)| {
                            let train = split.train.get(&user).map_or(&[][..], Vec::as_slice);
                            predict_user(
                                user,
                                hidden,
                                train,
                                collaborative,
                                details,
                                idf,
                                weights,
                                &mut signals,
                            )
                        })
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
    });

    let hidden: usize = split.test.values().map(Vec::len).sum();
    let all: Vec<&Predicted> = per_user.iter().flatten().collect();
    report(&per_user, &all, split, hidden)
}

/// Every method's predictions for one user's hidden films that `details` has.
#[allow(clippy::too_many_arguments)]
fn predict_user(
    user: u32,
    hidden: &[(u64, f32)],
    train: &[(u64, f32)],
    collaborative: &Collaborative,
    details: &BTreeMap<u64, MovieDetails>,
    idf: &Idf,
    weights: FieldWeights,
    signals: &mut Signals,
) -> Vec<Predicted> {
    let rated: Vec<RatedDetails> = train
        .iter()
        .filter_map(|(id, rating)| {
            Some(RatedDetails {
                movie: details.get(id)?.clone(),
                rating: *rating,
            })
        })
        .collect();
    if rated.is_empty() || train.is_empty() {
        return Vec::new();
    }
    let average = train.iter().map(|(_, r)| r).sum::<f32>() / train.len() as f32;
    let profile = TasteProfile::new(&rated, idf.clone(), weights);
    signals.shift = AudienceShift::fit(&rated, weights);
    hidden
        .iter()
        .filter_map(|&(id, rating)| {
            let movie = details.get(&id)?;
            let prediction = scoring::predict(&profile, &profile.vectorize(movie));
            let breakdown = scoring::breakdown(&prediction, movie, signals);
            Some(Predicted {
                movie: id,
                rating,
                popularity: collaborative.ratings_of(id),
                predictions: [
                    average,
                    collaborative.baseline(user, id),
                    collaborative.item_knn(user, id),
                    collaborative.svd(user, id),
                    breakdown.similarity,
                    breakdown.unreleased,
                    breakdown.rating,
                ],
            })
        })
        .collect()
}

fn report(
    per_user: &[Vec<Predicted>],
    all: &[&Predicted],
    split: &Split,
    hidden: usize,
) -> BenchmarkReport {
    let pairs = |group: &[&Predicted], m: usize| -> Vec<(f32, f32)> {
        group.iter().map(|p| (p.predictions[m], p.rating)).collect()
    };
    // "your average" guesses one number per user: it can't order anything.
    let orders = |m: usize| METHODS[m] != "your average";
    let per_user_mean = |score: &dyn Fn(&[Predicted], usize) -> Option<f32>, m: usize| {
        let values: Vec<f32> = per_user.iter().filter_map(|p| score(p, m)).collect();
        (orders(m) && !values.is_empty()).then(|| values.iter().sum::<f32>() / values.len() as f32)
    };
    let spearman = |p: &[Predicted], m: usize| {
        let (predicted, actual): (Vec<f32>, Vec<f32>) =
            p.iter().map(|x| (x.predictions[m], x.rating)).unzip();
        evaluate::spearman(&predicted, &actual)
    };
    let ndcg_of = |p: &[Predicted], m: usize| {
        let (predicted, actual): (Vec<f32>, Vec<f32>) =
            p.iter().map(|x| (x.predictions[m], x.rating)).unzip();
        ndcg(&predicted, &actual, NDCG_AT)
    };

    let by_popularity = POPULARITY
        .iter()
        .enumerate()
        .map(|(b, &(from, label))| {
            let to = POPULARITY.get(b + 1).map_or(usize::MAX, |(next, _)| *next);
            let group: Vec<&Predicted> = all
                .iter()
                .copied()
                .filter(|p| (from..to).contains(&p.popularity))
                .collect();
            PopularityBucket {
                label,
                ratings: group.len(),
                mae: (0..METHODS.len())
                    .map(|m| evaluate::summarize(METHODS[m], &pairs(&group, m), false).mae)
                    .collect(),
            }
        })
        .collect();

    BenchmarkReport {
        users: per_user.iter().filter(|p| !p.is_empty()).count(),
        train_ratings: split.train.values().map(Vec::len).sum(),
        predicted: all.len(),
        skipped: hidden - all.len(),
        overall: (0..METHODS.len())
            .map(|m| evaluate::summarize(METHODS[m], &pairs(all, m), false))
            .collect(),
        mean_spearman: (0..METHODS.len())
            .map(|m| per_user_mean(&spearman, m))
            .collect(),
        mean_ndcg: (0..METHODS.len())
            .map(|m| per_user_mean(&ndcg_of, m))
            .collect(),
        by_popularity,
    }
}

/// Normalised discounted cumulative gain of the top `k` when `actual` ratings are
/// ordered by `predicted`, 0 to 1: 1 when the highest rated come first. Ties keep the
/// given order. `None` for fewer than two films.
pub fn ndcg(predicted: &[f32], actual: &[f32], k: usize) -> Option<f32> {
    if predicted.len() < 2 || predicted.len() != actual.len() {
        return None;
    }
    let gain = |order: &[usize]| -> f32 {
        order
            .iter()
            .take(k)
            .enumerate()
            .map(|(rank, &i)| actual[i] / (rank as f32 + 2.0).log2())
            .sum()
    };
    let mut by_prediction: Vec<usize> = (0..predicted.len()).collect();
    by_prediction.sort_by(|&a, &b| predicted[b].total_cmp(&predicted[a]));
    let mut ideal: Vec<usize> = (0..actual.len()).collect();
    ideal.sort_by(|&a, &b| actual[b].total_cmp(&actual[a]));
    let best = gain(&ideal);
    (best > 0.0).then(|| gain(&by_prediction) / best)
}
