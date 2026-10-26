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
