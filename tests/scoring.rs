mod common;

use common::{
    favourites, filler_dramas, movie, profile, rated, Spec, COMEDY, DRAMA, HORROR, ROMANCE,
};
use movie_match::profile::TasteProfile;
use movie_match::scoring::{rank, score_detailed, Signals, OVERALL_SHRINK, SIMILARITY_POWER};
use movie_match::tmdb::MovieDetails;

fn comedy(id: u64, director: u64) -> MovieDetails {
    movie(
        "Comedy",
        Spec {
            id,
            genres: vec![COMEDY],
            keywords: vec![1],
            directors: vec![director],
            ..Spec::default()
        },
    )
}

fn horror(id: u64, director: u64) -> MovieDetails {
    movie(
        "Horror",
        Spec {
            id,
            genres: vec![HORROR],
            keywords: vec![2],
            directors: vec![director],
            ..Spec::default()
        },
    )
}

/// `movie`'s score from similarity alone.
fn score(profile: &TasteProfile, movie: &MovieDetails) -> u8 {
    score_detailed(profile, &Signals::default(), movie).score
}

#[test]
fn identical_favourite_scores_100() {
    let liked = movie(
        "Liked",
        Spec {
            genres: vec![DRAMA],
            keywords: vec![1, 2],
            directors: vec![7],
            cast: vec![20, 21],
            ..Spec::default()
        },
    );
    let p = profile(&[rated(&liked, 10.0)], &[], &filler_dramas(5));
    assert_eq!(score(&p, &liked), 100);
}

#[test]
fn score_is_the_predicted_rating_times_ten() {
    let seen = comedy(1, 10);
    let p = profile(&[rated(&seen, 7.0)], &[], &[]);
    assert_eq!(score(&p, &seen), 70);
}

#[test]
fn each_movie_is_predicted_from_its_own_similar_ratings() {
    // Loves comedies, dislikes horror.
    let rated_movies = [
        rated(&comedy(1, 10), 9.0),
        rated(&comedy(2, 11), 10.0),
        rated(&horror(3, 20), 3.0),
        rated(&horror(4, 21), 2.0),
    ];
    let new_comedy = comedy(5, 12);
    let new_horror = horror(6, 22);
    let candidates = [new_comedy.clone(), new_horror.clone()];
    let p = profile(&rated_movies, &candidates, &filler_dramas(5));

    let comedy_score = score_detailed(&p, &Signals::default(), &new_comedy);
    let horror_score = score_detailed(&p, &Signals::default(), &new_horror);
    let average = 60; // (9 + 10 + 3 + 2) / 4 × 10

    assert!(comedy_score.score > average, "{}", comedy_score.score);
    assert!(horror_score.score < average, "{}", horror_score.score);
    assert!(comedy_score.closest_disliked.is_none());
    assert!(horror_score.closest_liked.is_none());
    assert_eq!(
        horror_score.closest_disliked.map(|r| r.title.as_str()),
        Some("Horror")
    );
}

#[test]
fn neighbours_are_chosen_by_similarity_not_rating() {
    // A barely-similar 10/10 shouldn't outweigh a near-identical 3/10.
    let near_identical = horror(1, 20);
    let faint = movie(
        "Faint",
        Spec {
            id: 2,
            genres: vec![HORROR],
            directors: vec![30],
            ..Spec::default()
        },
    );
    let candidate = horror(3, 20);
    let p = profile(
        &[rated(&near_identical, 3.0), rated(&faint, 10.0)],
        std::slice::from_ref(&candidate),
        &filler_dramas(5),
    );

    // Average rating is 6.5, so a prediction below it means the 3/10 dominated.
    assert!(score(&p, &candidate) < 65);
}

#[test]
fn unrelated_movie_gets_your_average_rating() {
    let p = profile(
        &[rated(&comedy(1, 10), 9.0), rated(&comedy(2, 11), 3.0)],
        &[],
        &[],
    );
    let unrelated = movie(
        "Unrelated",
        Spec {
            genres: vec![ROMANCE],
            ..Spec::default()
        },
    );
    let found = score_detailed(&p, &Signals::default(), &unrelated);
    assert_eq!(found.score, 60);
    assert!(found.closest_liked.is_none());
    assert!(found.reasons.is_empty());
}

#[test]
fn rare_shared_features_count_more_than_common_ones() {
    // Two rated dramas: one 2/10 by director 7, one 10/10 by director 8.
    let drama = |id, director| {
        movie(
            "Drama",
            Spec {
                id,
                genres: vec![DRAMA],
                directors: vec![director],
                ..Spec::default()
            },
        )
    };
    let low = drama(1, 7);
    let high = drama(2, 8);
    // Each candidate shares the common Drama genre with both, and a (rare)
    // director with just one of them.
    let like_low = drama(3, 7);
    let like_high = drama(4, 8);
    let candidates = [like_low.clone(), like_high.clone()];
    let p = profile(
        &[rated(&low, 2.0), rated(&high, 10.0)],
        &candidates,
        &filler_dramas(20),
    );

    let toward_low = movie_match::scoring::explain(&p, &Signals::default(), &like_low);
    let toward_high = movie_match::scoring::explain(&p, &Signals::default(), &like_high);
    assert_eq!(toward_low.neighbours.len(), 2);
    // The shared director decides which rating dominates the weighted average.
    assert!(
        toward_low.neighbour_rating < 4.0,
        "{}",
        toward_low.neighbour_rating
    );
    assert!(
        toward_high.neighbour_rating > 8.0,
        "{}",
        toward_high.neighbour_rating
    );
    assert!(score(&p, &like_low) < score(&p, &like_high));
}

#[test]
fn reasons_list_the_strongest_shared_features_first() {
    let liked = movie(
        "Liked",
        Spec {
            genres: vec![DRAMA],
            keywords: vec![500],
            directors: vec![7],
            cast: vec![30],
            ..Spec::default()
        },
    );
    let candidate = movie(
        "Candidate",
        Spec {
            genres: vec![DRAMA],
            keywords: vec![500],
            directors: vec![7],
            ..Spec::default()
        },
    );
    let p = profile(
        &favourites(std::slice::from_ref(&liked)),
        std::slice::from_ref(&candidate),
        &filler_dramas(10),
    );

    let found = score_detailed(&p, &Signals::default(), &candidate);
    assert_eq!(
        found.reasons,
        ["director Director 7", "keyword \"kw500\"", "genre Genre 18"]
    );
}

#[test]
fn empty_profile_scores_zero() {
    let p = profile(&[], &[], &[]);
    assert_eq!(score(&p, &comedy(1, 10)), 0);
}

#[test]
fn rank_skips_rated_movies_and_sorts_best_first() {
    let liked = comedy(1, 10);
    let disliked = horror(2, 20);
    let candidates = [
        horror(3, 20),
        disliked.clone(),
        comedy(4, 10),
        liked.clone(),
    ];
    let p = profile(
        &[rated(&liked, 10.0), rated(&disliked, 2.0)],
        &candidates,
        &filler_dramas(5),
    );

    let titles: Vec<_> = rank(&p, &Signals::default(), &candidates)
        .into_iter()
        .map(|r| r.movie.id)
        .collect();
    assert_eq!(titles, [4, 3]);
}

#[test]
fn several_weak_matches_outweigh_the_average() {
    // Six 2/10 dramas and six 10/10 comedies: an overall average of 6.
    let dramas = (0..6).map(|i| {
        movie(
            "Disliked drama",
            Spec {
                id: 100 + i,
                genres: vec![DRAMA],
                directors: vec![50 + i],
                keywords: vec![70 + i],
                ..Spec::default()
            },
        )
    });
    let rated_movies: Vec<_> = dramas
        .map(|m| rated(&m, 2.0))
        .chain((0..6).map(|i| rated(&comedy(200 + i, 300 + i), 10.0)))
        .collect();
    // Shares only the common Drama genre with the six dramas.
    let candidate = movie(
        "Candidate",
        Spec {
            id: 2,
            genres: vec![DRAMA],
            ..Spec::default()
        },
    );
    let p = profile(
        &rated_movies,
        std::slice::from_ref(&candidate),
        &filler_dramas(10),
    );

    // (6 × 2 + 5 × 6) / (6 + 5) = 3.8: well below the average of 6.
    let found = score(&p, &candidate);
    assert_eq!(found, 38);
}

#[test]
fn explain_lists_neighbours_with_relative_weights() {
    let rated_movies = [rated(&comedy(1, 10), 9.0), rated(&comedy(2, 11), 7.0)];
    let candidate = comedy(3, 10);
    let p = profile(
        &rated_movies,
        std::slice::from_ref(&candidate),
        &filler_dramas(5),
    );

    let explanation = movie_match::scoring::explain(&p, &Signals::default(), &candidate);
    assert_eq!(explanation.score, score(&p, &candidate));
    assert_eq!(explanation.neighbours.len(), 2);
    let first = &explanation.neighbours[0];
    assert_eq!(first.rating, 9.0);
    assert_eq!(first.weight, 1.0);
    assert!(first.shared.contains(&"director Director 10".to_string()));
    assert!(explanation.neighbours[1].weight < 1.0);
}

#[test]
fn one_faint_neighbour_cannot_decide_the_score() {
    // Thirty rated movies averaging about 7. The candidate's only overlap is the
    // common Drama genre, shared with one 2/10 film among many dramas.
    let mut rated_movies: Vec<_> = (0..29)
        .map(|i| rated(&comedy(100 + i, 200 + i), 7.2))
        .collect();
    let bad = movie(
        "Bad",
        Spec {
            id: 1,
            genres: vec![DRAMA],
            keywords: (300..330).collect(),
            directors: vec![400],
            cast: vec![500, 501, 502, 503, 504],
        },
    );
    rated_movies.push(rated(&bad, 2.0));
    let candidate = movie(
        "Candidate",
        Spec {
            id: 2,
            genres: vec![DRAMA],
            keywords: vec![900, 901, 902],
            directors: vec![901],
            ..Spec::default()
        },
    );
    let p = profile(
        &rated_movies,
        std::slice::from_ref(&candidate),
        &filler_dramas(40),
    );

    let explanation = movie_match::scoring::explain(&p, &Signals::default(), &candidate);
    assert_eq!(explanation.neighbours.len(), 1);
    assert!(
        explanation.neighbours[0].similarity < 0.02,
        "test needs a faint match, got {}",
        explanation.neighbours[0].similarity
    );
    // With a single neighbour the prediction is mostly the overall average:
    // (2 + OVERALL_SHRINK × average) / (1 + OVERALL_SHRINK).
    let expected = (2.0 + OVERALL_SHRINK * explanation.mean_rating) / (1.0 + OVERALL_SHRINK);
    assert!(
        (explanation.predicted_rating - expected).abs() < 1e-4,
        "predicted {} vs expected {expected}",
        explanation.predicted_rating
    );
    assert!((explanation.overall_share - 5.0 / 6.0).abs() < 1e-6);
    // The 2/10 match pulls the score down a little, nowhere near 20.
    assert!(explanation.score >= 55, "score {}", explanation.score);
}

#[test]
fn faint_matches_fall_back_on_the_neighbours_ratings_not_every_rating() {
    // Ten dramas rated 9 and thirty comedies rated 3: an overall average of 4.5.
    let dramas = (0..10).map(|i| {
        movie(
            "Drama",
            Spec {
                id: 100 + i,
                genres: vec![DRAMA],
                keywords: vec![300 + i],
                directors: vec![400 + i],
                ..Spec::default()
            },
        )
    });
    let rated_movies: Vec<_> = dramas
        .map(|m| rated(&m, 9.0))
        .chain((0..30).map(|i| rated(&comedy(500 + i, 600 + i), 3.0)))
        .collect();
    // The candidate shares only the common Drama genre with the rated dramas.
    let candidate = movie(
        "Candidate",
        Spec {
            id: 2,
            genres: vec![DRAMA],
            keywords: vec![900],
            directors: vec![901],
            ..Spec::default()
        },
    );
    let p = profile(
        &rated_movies,
        std::slice::from_ref(&candidate),
        &filler_dramas(40),
    );

    let explanation = movie_match::scoring::explain(&p, &Signals::default(), &candidate);
    assert_eq!(explanation.neighbours.len(), 10);
    assert!(
        explanation.neighbours[0].similarity < 0.08,
        "test needs faint matches, got {}",
        explanation.neighbours[0].similarity
    );
    assert!((explanation.mean_rating - 4.5).abs() < 1e-4);
    assert!((explanation.neighbour_rating - 9.0).abs() < 1e-4);
    // (10 × 9 + 5 × 4.5) / (10 + 5): the dramas' ratings, not everything rated.
    assert!(
        (explanation.predicted_rating - 7.5).abs() < 1e-4,
        "predicted {}",
        explanation.predicted_rating
    );
    assert_eq!(explanation.score, 75);
}

#[test]
fn more_tied_faint_matches_than_neighbours_all_count_whatever_the_file_order() {
    // Sixty dramas: the first thirty rated 9, the last thirty rated 3.
    let dramas: Vec<_> = (0..60)
        .map(|i| {
            movie(
                "Drama",
                Spec {
                    id: 100 + i,
                    genres: vec![DRAMA],
                    keywords: vec![300 + i],
                    directors: vec![400 + i],
                    ..Spec::default()
                },
            )
        })
        .collect();
    let rated_movies: Vec<_> = dramas
        .iter()
        .enumerate()
        .map(|(i, m)| rated(m, if i < 30 { 9.0 } else { 3.0 }))
        .collect();
    let mut reversed = rated_movies.clone();
    reversed.reverse();
    // The candidate shares only the common Drama genre with every rated drama.
    let candidate = movie(
        "Candidate",
        Spec {
            id: 2,
            genres: vec![DRAMA],
            keywords: vec![900],
            directors: vec![901],
            ..Spec::default()
        },
    );
    let comedies: Vec<_> = (0..20).map(|i| comedy(700 + i, 800 + i)).collect();
    let explain = |rated_movies: &[_]| {
        let p = profile(rated_movies, std::slice::from_ref(&candidate), &comedies);
        movie_match::scoring::explain(&p, &Signals::default(), &candidate)
    };
    let in_order = explain(&rated_movies);
    let in_reverse = explain(&reversed);

    // All sixty are tied, so all are kept, splitting the 20 places three ways.
    assert_eq!(in_order.neighbours.len(), 60);
    let first = &in_order.neighbours[0];
    assert!(in_order
        .neighbours
        .iter()
        .all(|n| n.similarity == first.similarity));
    assert!(first.similarity < 0.08, "faint: {}", first.similarity);
    let high = in_order.neighbours.iter().find(|n| n.rating == 9.0);
    assert!((high.map_or(0.0, |n| n.weight) - 1.0 / 3.0).abs() < 1e-5);
    // Every drama counts equally, not just the first twenty 9s, which would
    // predict (20 × 9 + 5 × 6) / 25 = 8.4.
    assert!((in_order.neighbour_rating - 6.0).abs() < 1e-4);
    // Still counted as 20 neighbours for the overall average's share.
    assert!((in_order.overall_share - OVERALL_SHRINK / 25.0).abs() < 1e-6);
    assert!(
        (in_order.predicted_rating - 6.0).abs() < 1e-4,
        "predicted {}",
        in_order.predicted_rating
    );
    assert_eq!(in_order.score, 60);
    // Reordering the same ratings changes nothing.
    assert!((in_reverse.predicted_rating - in_order.predicted_rating).abs() < 1e-5);
    assert_eq!(in_reverse.score, in_order.score);
}

#[test]
fn movies_tied_for_the_last_neighbour_place_split_it() {
    // One close 10/10 drama sharing a director, then 25 dramas sharing only the
    // genre, tied for the 19 places left.
    let close = movie(
        "Close",
        Spec {
            id: 1,
            genres: vec![DRAMA],
            directors: vec![901],
            ..Spec::default()
        },
    );
    let faint = (0..25).map(|i| {
        movie(
            "Faint",
            Spec {
                id: 100 + i,
                genres: vec![DRAMA],
                keywords: vec![300 + i],
                directors: vec![400 + i],
                ..Spec::default()
            },
        )
    });
    let rated_movies: Vec<_> = std::iter::once(rated(&close, 10.0))
        .chain(faint.map(|m| rated(&m, 8.0)))
        .collect();
    let candidate = movie(
        "Candidate",
        Spec {
            id: 2,
            genres: vec![DRAMA],
            keywords: vec![900],
            directors: vec![901],
            ..Spec::default()
        },
    );
    let comedies: Vec<_> = (0..20).map(|i| comedy(700 + i, 800 + i)).collect();
    let p = profile(&rated_movies, std::slice::from_ref(&candidate), &comedies);
    let explanation = movie_match::scoring::explain(&p, &Signals::default(), &candidate);

    assert_eq!(explanation.neighbours.len(), 26);
    assert_eq!(explanation.neighbours[0].title, "Close");
    assert!((explanation.neighbours[0].weight - 1.0).abs() < 1e-6);
    let top = explanation.neighbours[0].similarity;
    for n in &explanation.neighbours[1..] {
        let expected = (n.similarity / top).powi(SIMILARITY_POWER) * 19.0 / 25.0;
        assert!((n.weight - expected).abs() < 1e-6, "weight {}", n.weight);
    }
    assert!((explanation.overall_share - OVERALL_SHRINK / 25.0).abs() < 1e-6);
}

#[test]
fn closer_neighbours_count_by_similarity_squared() {
    // A 10/10 movie sharing a director and a genre with the candidate, and a 2/10
    // movie sharing only the genre.
    let close = movie(
        "Close",
        Spec {
            id: 1,
            genres: vec![DRAMA],
            directors: vec![7],
            ..Spec::default()
        },
    );
    let far = movie(
        "Far",
        Spec {
            id: 2,
            genres: vec![DRAMA],
            directors: vec![8],
            ..Spec::default()
        },
    );
    let candidate = movie(
        "Candidate",
        Spec {
            id: 3,
            genres: vec![DRAMA],
            directors: vec![7],
            ..Spec::default()
        },
    );
    let p = profile(
        &[rated(&close, 10.0), rated(&far, 2.0)],
        std::slice::from_ref(&candidate),
        &filler_dramas(20),
    );

    let explanation = movie_match::scoring::explain(&p, &Signals::default(), &candidate);
    let [first, second] = explanation.neighbours.as_slice() else {
        panic!("expected two neighbours");
    };
    assert_eq!(first.title, "Close");
    let ratio = second.similarity / first.similarity;
    let expected = ratio.powi(SIMILARITY_POWER);
    assert!(
        (second.weight - expected).abs() < 1e-5,
        "weight {} vs {expected}",
        second.weight
    );
    let weighted = (10.0 + 2.0 * expected) / (1.0 + expected);
    assert!((explanation.neighbour_rating - weighted).abs() < 1e-4);
}

#[test]
fn equally_similar_ratings_count_the_same_however_low() {
    // Equally similar to a 10/10 and a 1/10, so the prediction is the midpoint.
    let good = comedy(1, 10);
    let bad = comedy(2, 20);
    let candidate = movie(
        "Candidate",
        Spec {
            id: 3,
            genres: vec![COMEDY],
            keywords: vec![1],
            ..Spec::default()
        },
    );
    let p = profile(
        &[rated(&good, 10.0), rated(&bad, 1.0)],
        std::slice::from_ref(&candidate),
        &filler_dramas(5),
    );

    let explanation = movie_match::scoring::explain(&p, &Signals::default(), &candidate);
    let weights: Vec<_> = explanation.neighbours.iter().map(|n| n.weight).collect();
    assert_eq!(weights.len(), 2);
    assert!((weights[1] - weights[0]).abs() < 1e-6, "{weights:?}");
    assert!(
        (explanation.predicted_rating - 5.5).abs() < 1e-4,
        "predicted {}",
        explanation.predicted_rating
    );
}
