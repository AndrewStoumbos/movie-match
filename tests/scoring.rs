mod common;

use common::{
    favourites, filler_dramas, movie, profile, rated, Spec, COMEDY, DRAMA, HORROR, ROMANCE,
};
use movie_match::scoring::{rank, score, score_detailed};
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

    let comedy_score = score_detailed(&p, &new_comedy);
    let horror_score = score_detailed(&p, &new_horror);
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
    let found = score_detailed(&p, &unrelated);
    assert_eq!(found.score, 60);
    assert!(found.closest_liked.is_none());
    assert!(found.reasons.is_empty());
}

#[test]
fn rare_shared_features_count_more_than_common_ones() {
    let liked = [movie(
        "Liked",
        Spec {
            genres: vec![DRAMA],
            keywords: vec![500],
            directors: vec![7],
            ..Spec::default()
        },
    )];
    let same_genre = movie(
        "Same genre",
        Spec {
            genres: vec![DRAMA],
            directors: vec![8],
            ..Spec::default()
        },
    );
    let same_director = movie(
        "Same director",
        Spec {
            genres: vec![COMEDY],
            directors: vec![7],
            ..Spec::default()
        },
    );
    let candidates = [same_genre.clone(), same_director.clone()];
    // A low rating, so closer resemblance means a lower predicted score.
    let p = profile(
        &[rated(&liked[0], 2.0), rated(&horror(9, 99), 10.0)],
        &candidates,
        &filler_dramas(20),
    );

    assert!(score(&p, &same_director) < score(&p, &same_genre));
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

    let found = score_detailed(&p, &candidate);
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

    let titles: Vec<_> = rank(&p, &candidates)
        .into_iter()
        .map(|r| r.movie.id)
        .collect();
    assert_eq!(titles, [4, 3]);
}

#[test]
fn weak_matches_still_outweigh_the_average() {
    // Shares only a common genre with a 2/10 movie; other ratings are high.
    let disliked = movie(
        "Disliked",
        Spec {
            id: 1,
            genres: vec![DRAMA],
            directors: vec![50],
            keywords: vec![70, 71, 72],
            ..Spec::default()
        },
    );
    let candidate = movie(
        "Candidate",
        Spec {
            id: 2,
            genres: vec![DRAMA],
            ..Spec::default()
        },
    );
    let p = profile(
        &[
            rated(&disliked, 2.0),
            rated(&comedy(3, 10), 10.0),
            rated(&comedy(4, 11), 10.0),
        ],
        std::slice::from_ref(&candidate),
        &filler_dramas(10),
    );

    // Average is 7.33, but the single 2/10 neighbour carries more weight than the
    // average (which counts 2 x (1 - its similarity)), so the score lands well below.
    let found = score(&p, &candidate);
    assert!(found < 60, "{found}");
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

    let explanation = movie_match::scoring::explain(&p, &candidate);
    assert_eq!(explanation.score, score(&p, &candidate));
    assert_eq!(explanation.neighbours.len(), 2);
    let first = &explanation.neighbours[0];
    assert_eq!(first.rating, 9.0);
    assert_eq!(first.weight, 1.0);
    assert!(first.shared.contains(&"director Director 10".to_string()));
    assert!(explanation.neighbours[1].weight < 1.0);
}

#[test]
fn one_faint_neighbour_barely_moves_the_score_from_the_average() {
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

    let explanation = movie_match::scoring::explain(&p, &candidate);
    assert_eq!(explanation.neighbours.len(), 1);
    assert!(
        explanation.neighbours[0].similarity < 0.02,
        "test needs a faint match, got {}",
        explanation.neighbours[0].similarity
    );
    let average = (explanation.mean_rating * 10.0).round() as i32;
    let drop = average - i32::from(explanation.score);
    // Without the confidence ramp this match would pull the score down ~17 points.
    assert!(
        drop <= 4,
        "score {} vs average {average}",
        explanation.score
    );
}

#[test]
fn bad_ratings_count_less_than_equally_similar_good_ones() {
    // Equally similar to a 10/10 and a 1/10: without the reduced impact the
    // prediction would be exactly the midpoint, 5.5.
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

    let explanation = movie_match::scoring::explain(&p, &candidate);
    let weights: Vec<_> = explanation.neighbours.iter().map(|n| n.weight).collect();
    assert_eq!(weights.len(), 2);
    assert!((weights[1] - 0.7 * weights[0]).abs() < 1e-5, "{weights:?}");
    assert!(explanation.predicted_rating > 5.5);
}

#[test]
fn low_ratings_are_the_users_bottom_forty_percent() {
    let ratings = [5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 10.0, 10.0, 10.0, 10.0];
    let rated_movies: Vec<_> = ratings
        .iter()
        .zip(0..)
        .map(|(&r, i)| rated(&comedy(i, 100 + i), r))
        .collect();
    let p = profile(&rated_movies, &[], &[]);

    // The rating at position ceil(0.4 × 10) = 4 is 9, so 5 to 8 are low.
    assert_eq!(p.low_rating_cutoff(), 9.0);
    assert!(p.is_low_rating(8.0));
    assert!(p.is_low_rating(6.0));
    assert!(!p.is_low_rating(9.0));
}

#[test]
fn ties_at_the_cutoff_are_not_low() {
    // Mostly 8s: the cutoff lands on 8, and only the 6 is below it.
    let ratings = [6.0, 8.0, 8.0, 8.0, 8.0];
    let rated_movies: Vec<_> = ratings
        .iter()
        .zip(0..)
        .map(|(&r, i)| rated(&comedy(i, 100 + i), r))
        .collect();
    let p = profile(&rated_movies, &[], &[]);

    assert_eq!(p.low_rating_cutoff(), 8.0);
    assert!(p.is_low_rating(6.0));
    assert!(!p.is_low_rating(8.0));
}
