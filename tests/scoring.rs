mod common;

use common::{
    favourites, filler_dramas, movie, profile, rated, Spec, COMEDY, DRAMA, HORROR, ROMANCE,
};
use movie_match::scoring::{rank, score, score_detailed};

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
fn rating_scales_how_much_a_liked_movie_counts() {
    let liked = movie(
        "Liked",
        Spec {
            genres: vec![DRAMA],
            directors: vec![7],
            ..Spec::default()
        },
    );
    let p = profile(&[rated(&liked, 7.0)], &[], &[]);
    assert_eq!(score(&p, &liked), 70);
}

#[test]
fn neutral_ratings_do_not_count() {
    let seen = movie(
        "Seen",
        Spec {
            genres: vec![DRAMA],
            directors: vec![7],
            ..Spec::default()
        },
    );
    let p = profile(&[rated(&seen, 5.0)], &[], &[]);
    let found = score_detailed(&p, &seen);
    assert_eq!(found.score, 0);
    assert!(found.closest_liked.is_none());
    assert!(found.closest_disliked.is_none());
}

#[test]
fn shared_director_beats_shared_common_genre() {
    let liked = [movie(
        "Liked",
        Spec {
            genres: vec![DRAMA],
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
    let p = profile(&favourites(&liked), &candidates, &filler_dramas(20));

    assert!(score(&p, &same_director) > score(&p, &same_genre));
}

#[test]
fn rare_keyword_beats_common_genre() {
    let liked = [movie(
        "Liked",
        Spec {
            genres: vec![DRAMA],
            keywords: vec![500],
            ..Spec::default()
        },
    )];
    let same_genre = movie(
        "Same genre",
        Spec {
            genres: vec![DRAMA],
            ..Spec::default()
        },
    );
    let same_keyword = movie(
        "Same keyword",
        Spec {
            genres: vec![COMEDY],
            keywords: vec![500],
            ..Spec::default()
        },
    );
    let candidates = [same_genre.clone(), same_keyword.clone()];
    let p = profile(&favourites(&liked), &candidates, &filler_dramas(20));

    assert!(score(&p, &same_keyword) > score(&p, &same_genre));
}

#[test]
fn eclectic_tastes_are_not_averaged_together() {
    let horror = movie(
        "Horror",
        Spec {
            genres: vec![HORROR],
            keywords: vec![1],
            directors: vec![10],
            ..Spec::default()
        },
    );
    let romcom = movie(
        "Romcom",
        Spec {
            genres: vec![ROMANCE, COMEDY],
            keywords: vec![2],
            directors: vec![20],
            ..Spec::default()
        },
    );
    let candidate = movie(
        "New romcom",
        Spec {
            genres: vec![ROMANCE, COMEDY],
            keywords: vec![2],
            ..Spec::default()
        },
    );
    let candidates = std::slice::from_ref(&candidate);
    let reference = filler_dramas(10);

    // The IDF set is the same for both profiles; only the rated list differs.
    let mixed = profile(
        &favourites(&[horror.clone(), romcom.clone()]),
        candidates,
        &reference,
    );
    let romcom_only = profile(
        &favourites(std::slice::from_ref(&romcom)),
        candidates,
        &[reference.clone(), vec![horror]].concat(),
    );

    let found = score_detailed(&mixed, &candidate);
    assert_eq!(
        found.closest_liked.map(|r| r.title.as_str()),
        Some("Romcom")
    );
    assert_eq!(found.score, score(&romcom_only, &candidate));
    assert!(found.score >= 50, "score was {}", found.score);
}

#[test]
fn resembling_a_disliked_movie_lowers_the_score() {
    let liked = movie(
        "Liked",
        Spec {
            genres: vec![COMEDY],
            keywords: vec![1],
            directors: vec![10],
            ..Spec::default()
        },
    );
    let disliked = movie(
        "Disliked",
        Spec {
            genres: vec![COMEDY],
            keywords: vec![2],
            directors: vec![20],
            ..Spec::default()
        },
    );
    let candidate = movie(
        "Candidate",
        Spec {
            genres: vec![COMEDY],
            keywords: vec![1, 2],
            ..Spec::default()
        },
    );
    let candidates = std::slice::from_ref(&candidate);
    let extra = filler_dramas(5);

    // Rated 5/10 is neutral, so both profiles use the same IDF set.
    let without = profile(
        &[rated(&liked, 10.0), rated(&disliked, 5.0)],
        candidates,
        &extra,
    );
    let with = profile(
        &[rated(&liked, 10.0), rated(&disliked, 1.0)],
        candidates,
        &extra,
    );

    let found = score_detailed(&with, &candidate);
    assert!(found.score < score(&without, &candidate));
    assert_eq!(
        found.closest_disliked.map(|r| r.title.as_str()),
        Some("Disliked")
    );
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
fn nothing_in_common_scores_zero() {
    let liked = [movie(
        "Liked",
        Spec {
            genres: vec![HORROR],
            ..Spec::default()
        },
    )];
    let candidate = movie(
        "Other",
        Spec {
            genres: vec![COMEDY],
            ..Spec::default()
        },
    );
    let p = profile(&favourites(&liked), &[], &[]);
    let found = score_detailed(&p, &candidate);
    assert_eq!(found.score, 0);
    assert!(found.closest_liked.is_none());
    assert!(found.reasons.is_empty());
}

#[test]
fn empty_profile_scores_zero() {
    let candidate = movie(
        "Any",
        Spec {
            genres: vec![DRAMA],
            ..Spec::default()
        },
    );
    let p = profile(&[], &[], &[]);
    assert_eq!(score(&p, &candidate), 0);
}

#[test]
fn movie_without_features_scores_zero() {
    let liked = [movie(
        "Liked",
        Spec {
            genres: vec![DRAMA],
            ..Spec::default()
        },
    )];
    let p = profile(&favourites(&liked), &[], &[]);
    assert_eq!(score(&p, &movie("Blank", Spec::default())), 0);
}

#[test]
fn rank_skips_rated_movies_and_sorts_best_first() {
    let liked = movie(
        "Liked",
        Spec {
            id: 1,
            genres: vec![DRAMA],
            directors: vec![7],
            ..Spec::default()
        },
    );
    let seen = movie(
        "Seen",
        Spec {
            id: 2,
            genres: vec![DRAMA],
            directors: vec![7],
            ..Spec::default()
        },
    );
    let weak = movie(
        "Weak match",
        Spec {
            id: 3,
            genres: vec![DRAMA],
            ..Spec::default()
        },
    );
    let strong = movie(
        "Strong match",
        Spec {
            id: 4,
            genres: vec![DRAMA],
            directors: vec![7],
            ..Spec::default()
        },
    );
    let candidates = [weak, seen.clone(), strong, liked.clone()];
    let p = profile(
        &[rated(&liked, 10.0), rated(&seen, 6.0)],
        &candidates,
        &filler_dramas(5),
    );

    let titles: Vec<_> = rank(&p, &candidates)
        .into_iter()
        .map(|r| r.movie.title)
        .collect();
    assert_eq!(titles, ["Strong match", "Weak match"]);
}
