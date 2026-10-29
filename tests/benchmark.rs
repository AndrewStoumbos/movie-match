//! The MovieLens benchmark's split, collaborative methods and report, on small
//! planted rating matrices.

mod common;

use std::collections::BTreeMap;

use common::{movie, Spec, COMEDY, DRAMA};
use movie_match::benchmark::{self, ndcg, split, Collaborative, METHODS};
use movie_match::features::{self, FieldWeights};
use movie_match::movielens::UserRatings;
use movie_match::tfidf::Idf;
use movie_match::tmdb::MovieDetails;

/// Users whose ratings are `rating(user, movie)` for every movie in `movies`, newest
/// first in movie order.
fn matrix(users: u32, movies: u64, rating: impl Fn(u32, u64) -> f32) -> UserRatings {
    (0..users)
        .map(|u| (u, (0..movies).map(|m| (m, rating(u, m))).collect()))
        .collect()
}

fn index(method: &str) -> usize {
    METHODS
        .iter()
        .position(|m| *m == method)
        .expect("known method")
}

#[test]
fn split_hides_each_users_newest_fifth_and_keeps_enough_to_learn_from() {
    let mut users = UserRatings::new();
    users.insert(1, (0..10).map(|m| (m, 8.0)).collect());
    // 6 ratings: hiding 2 would leave 4, under the minimum of 5.
    users.insert(2, (0..6).map(|m| (m, 8.0)).collect());

    let split = split(&users, 0.2, 5);

    let hidden: Vec<u64> = split.test[&1].iter().map(|(m, _)| *m).collect();
    assert_eq!(hidden, [0, 1], "the newest come first");
    assert_eq!(split.train[&1].len(), 8);
    assert!(!split.test.contains_key(&2));
    assert_eq!(
        split.train[&2].len(),
        6,
        "still trains the collaborative methods"
    );
}

#[test]
fn hidden_ratings_never_reach_the_collaborative_methods() {
    let users = matrix(20, 30, |u, m| 1.0 + ((u as u64 * 7 + m * 3) % 10) as f32);
    let mut changed = users.clone();
    for ratings in changed.values_mut() {
        // The newest 20% are hidden: rewrite them to an extreme.
        for rating in ratings.iter_mut().take(6) {
            rating.1 = 10.0;
        }
    }

    let a = Collaborative::fit(&split(&users, 0.2, 5).train, 7);
    let b = Collaborative::fit(&split(&changed, 0.2, 5).train, 7);

    for (user, movie) in [(0, 0), (3, 2), (19, 5)] {
        assert_eq!(a.baseline(user, movie), b.baseline(user, movie));
        assert_eq!(a.item_knn(user, movie), b.item_knn(user, movie));
        assert_eq!(a.svd(user, movie), b.svd(user, movie));
    }
}

#[test]
fn baseline_recovers_planted_user_and_film_biases() {
    // Generous and harsh users, good and bad films, nothing else.
    let user_bias = |u: u32| if u.is_multiple_of(2) { 1.5 } else { -1.5 };
    let film_bias = |m: u64| if m.is_multiple_of(2) { 1.0 } else { -1.0 };
    // Enough ratings each that the regularisation barely shrinks the biases.
    let mut users = matrix(200, 200, |u, m| 5.5 + user_bias(u) + film_bias(m));
    // Hide (0, 0) and (1, 1) by leaving them out of training.
    users.get_mut(&0).expect("user 0").retain(|(m, _)| *m != 0);
    users.get_mut(&1).expect("user 1").retain(|(m, _)| *m != 1);

    let fitted = Collaborative::fit(&users, 1);

    assert!(
        (fitted.baseline(0, 0) - 8.0).abs() < 0.3,
        "{}",
        fitted.baseline(0, 0)
    );
    assert!(
        (fitted.baseline(1, 1) - 3.0).abs() < 0.3,
        "{}",
        fitted.baseline(1, 1)
    );
}

#[test]
fn item_knn_follows_a_film_rated_like_the_hidden_one() {
    // Films 0 and 1 are rated alike, and unlike the rest, by everyone.
    let mut users = matrix(30, 10, |u, m| match m {
        0 | 1 => 1.0 + (u % 10) as f32,
        _ => 5.0 + ((u as u64 + m) % 3) as f32,
    });
    // User 99 loved film 1 and hasn't seen film 0.
    users.insert(
        99,
        (1..10)
            .map(|m| (m, if m == 1 { 10.0 } else { 5.0 }))
            .collect(),
    );

    let fitted = Collaborative::fit(&users, 1);

    assert!(
        fitted.item_knn(99, 0) > fitted.baseline(99, 0) + 1.0,
        "k-NN {} vs baseline {}",
        fitted.item_knn(99, 0),
        fitted.baseline(99, 0)
    );
}

#[test]
fn svd_repeats_with_its_seed_and_learns_tastes_biases_cannot() {
    // Two taste groups that disagree about two film groups: no user or film bias
    // explains it, only their interaction.
    let taste = |u: u32, m: u64| {
        if u.is_multiple_of(2) == m.is_multiple_of(2) {
            8.0
        } else {
            3.0
        }
    };
    let mut users = matrix(60, 60, taste);
    let hidden: Vec<(u32, u64)> = (0..60).map(|u| (u, (u as u64 * 7) % 60)).collect();
    for &(u, m) in &hidden {
        users
            .get_mut(&u)
            .expect("user")
            .retain(|(movie, _)| *movie != m);
    }

    let fitted = Collaborative::fit(&users, 3);
    let again = Collaborative::fit(&users, 3);
    let mae = |predict: &dyn Fn(u32, u64) -> f32| {
        hidden
            .iter()
            .map(|&(u, m)| (predict(u, m) - taste(u, m)).abs())
            .sum::<f32>()
            / hidden.len() as f32
    };

    assert_eq!(fitted.svd(5, 35), again.svd(5, 35));
    let svd = mae(&|u, m| fitted.svd(u, m));
    let baseline = mae(&|u, m| fitted.baseline(u, m));
    assert!(svd < baseline - 0.5, "SVD {svd} vs baseline {baseline}");
}

#[test]
fn a_film_nobody_rated_gets_the_users_baseline() {
    let users = matrix(10, 10, |u, _| 4.0 + u as f32 * 0.5);
    let fitted = Collaborative::fit(&users, 1);

    assert_eq!(fitted.ratings_of(999), 0);
    assert_eq!(fitted.item_knn(4, 999), fitted.baseline(4, 999));
    assert!(fitted.baseline(9, 999) > fitted.baseline(0, 999));
}

#[test]
fn ndcg_is_one_for_the_right_order_and_less_otherwise() {
    let actual = [9.0, 6.0, 3.0];
    assert_eq!(ndcg(&[3.0, 2.0, 1.0], &actual, 10), Some(1.0));
    let reversed = ndcg(&[1.0, 2.0, 3.0], &actual, 10).expect("ndcg");
    assert!(reversed < 0.9, "{reversed}");
    assert_eq!(ndcg(&[1.0], &[5.0], 10), None);
}

#[test]
fn run_scores_hidden_films_with_details_and_buckets_them_by_popularity() {
    // Ten dramas and ten comedies; film 99 is rated only as a user's newest rating.
    let films: Vec<MovieDetails> = (0..20)
        .map(|id| {
            movie(
                &format!("Film {id}"),
                Spec {
                    id,
                    genres: vec![if id < 10 { DRAMA } else { COMEDY }],
                    ..Spec::default()
                },
            )
        })
        .chain([movie(
            "New drama",
            Spec {
                id: 99,
                genres: vec![DRAMA],
                ..Spec::default()
            },
        )])
        .collect();
    let mut users = UserRatings::new();
    for u in 0..6u32 {
        // Drama fans: dramas 9, comedies 3. Newest first: film 99 (or a film TMDB
        // lacks, 500), then the rest in an order that differs per user, so each
        // user's other hidden films are in someone else's training ratings.
        let newest = if u == 0 { 500 } else { 99 };
        let mut ratings = vec![(newest, 9.0)];
        ratings.extend(
            (0..20)
                .map(|i| (i + 3 * u64::from(u)) % 20)
                .map(|id| (id, if id < 10 { 9.0 } else { 3.0 })),
        );
        users.insert(u, ratings);
    }
    let details: BTreeMap<u64, MovieDetails> = films.into_iter().map(|m| (m.id, m)).collect();
    let weights = FieldWeights::default();
    let documents: Vec<_> = details
        .values()
        .map(|m| features::extract(m, &weights))
        .collect();
    let idf = Idf::from_documents(&documents);

    let report = benchmark::run(&split(&users, 0.2, 5), &details, &idf, weights, None, 1);

    // 21 ratings each: 5 hidden (the newest film and the next 4).
    assert_eq!(report.users, 6);
    assert_eq!(report.skipped, 1, "film 500 has no details");
    assert_eq!(report.predicted, 29);
    let unseen = &report.by_popularity[0];
    assert_eq!(unseen.label, "0");
    assert_eq!(unseen.ratings, 5, "film 99 is never in training");
    // Movie Match knows film 99 is a drama; the collaborative methods know nothing
    // about a film nobody rated.
    let similarity = unseen.mae[index("similarity only")];
    let svd = unseen.mae[index("SVD")];
    assert!(similarity < svd, "similarity {similarity} vs SVD {svd}");
    assert_eq!(report.overall.len(), METHODS.len());
    assert_eq!(report.mean_ndcg[index("your average")], None);
}
