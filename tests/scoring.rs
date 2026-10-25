mod common;

use common::{filler_dramas, movie, Spec, COMEDY, DRAMA, HORROR, ROMANCE};
use movie_match::features::{self, FieldWeights};
use movie_match::profile::TasteProfile;
use movie_match::scoring::{score, score_detailed};
use movie_match::tfidf::Idf;
use movie_match::tmdb::MovieDetails;

/// A profile whose IDF comes from `liked`, `candidates` and `extra` together.
fn profile(
    liked: &[MovieDetails],
    candidates: &[MovieDetails],
    extra: &[MovieDetails],
) -> TasteProfile {
    let weights = FieldWeights::default();
    let documents: Vec<_> = liked
        .iter()
        .chain(candidates)
        .chain(extra)
        .map(|m| features::extract(m, &weights))
        .collect();
    TasteProfile::new(liked, Idf::from_documents(&documents), weights)
}

#[test]
fn identical_movie_scores_100() {
    let liked = movie(
        "Liked",
        Spec {
            genres: vec![DRAMA],
            keywords: vec![1, 2],
            directors: vec![7],
            cast: vec![20, 21],
        },
    );
    let p = profile(std::slice::from_ref(&liked), &[], &filler_dramas(5));
    assert_eq!(score(&p, &liked), 100);
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
    let p = profile(&liked, &candidates, &filler_dramas(20));

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
    let p = profile(&liked, &candidates, &filler_dramas(20));

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
    let liked_both = [horror.clone(), romcom.clone()];

    // The IDF set is the same for both profiles; only the liked list differs.
    let mixed = profile(&liked_both, candidates, &reference);
    let romcom_only = profile(
        std::slice::from_ref(&romcom),
        candidates,
        &[reference.clone(), vec![horror]].concat(),
    );

    let found = score_detailed(&mixed, &candidate);
    assert_eq!(found.closest_liked, Some("Romcom"));
    assert_eq!(found.score, score(&romcom_only, &candidate));
    assert!(found.score >= 50, "score was {}", found.score);
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
    let p = profile(&liked, &[], &[]);
    let found = score_detailed(&p, &candidate);
    assert_eq!(found.score, 0);
    assert_eq!(found.closest_liked, None);
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
    let p = profile(&liked, &[], &[]);
    assert_eq!(score(&p, &movie("Blank", Spec::default())), 0);
}
