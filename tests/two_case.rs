//! The audience shift and the two-case scoring: unreleased films from similarity and
//! track records, released ones from the audience score and the user's shift.

mod common;

use std::collections::HashMap;

use common::{movie, profile, rated, Spec, COMEDY, DRAMA, HORROR};
use movie_match::features::FieldWeights;
use movie_match::profile::RatedDetails;
use movie_match::scoring::{
    self, audience_share, blend, nudged, Signals, TrackRecords, AUDIENCE_VOTES_HALF,
};
use movie_match::shift::AudienceShift;
use movie_match::tmdb::MovieDetails;

/// A film with `votes` audience votes averaging `score`.
fn voted(mut movie: MovieDetails, score: f32, votes: u32) -> MovieDetails {
    movie.vote_average = Some(score);
    movie.vote_count = Some(votes);
    movie
}

/// Six rated films with 1,000 votes each: three by director 7, which the user rates
/// two points above the audience, and three dramas by others, rated at the audience.
fn director_fan() -> Vec<RatedDetails> {
    (0..6u64)
        .map(|i| {
            let by_seven = i < 3;
            let film = movie(
                &format!("Film {i}"),
                Spec {
                    id: i + 1,
                    genres: vec![if by_seven { HORROR } else { DRAMA }],
                    directors: vec![if by_seven { 7 } else { 100 + i }],
                    ..Spec::default()
                },
            );
            let audience = 6.0;
            let rating = if by_seven { audience + 2.0 } else { audience };
            rated(&voted(film, audience, 1000), rating)
        })
        .collect()
}

#[test]
fn shift_learns_a_director_the_user_rates_above_the_crowd() {
    let shift = AudienceShift::fit(&director_fan(), FieldWeights::default()).expect("votes");
    let next_by_seven = movie(
        "Next",
        Spec {
            id: 50,
            genres: vec![COMEDY],
            directors: vec![7],
            ..Spec::default()
        },
    );
    let unrelated = movie(
        "Other",
        Spec {
            id: 51,
            genres: vec![COMEDY],
            directors: vec![99],
            ..Spec::default()
        },
    );

    let by_seven = shift.predict(&next_by_seven);
    let other = shift.predict(&unrelated);
    // Average difference: (3 × 2 + 3 × 0) / 6.
    assert!((by_seven.average - 1.0).abs() < 1e-5);
    assert!(by_seven.features > 0.3, "{by_seven:?}");
    assert_eq!(by_seven.reasons[0].0, "director Director 7");
    // Nothing known about this one: just the average.
    assert!(other.features.abs() < 1e-5, "{other:?}");
    assert!(other.reasons.is_empty());
}

#[test]
fn no_shift_without_audience_votes() {
    let unvoted = vec![rated(
        &movie(
            "Festival film",
            Spec {
                id: 1,
                genres: vec![DRAMA],
                ..Spec::default()
            },
        ),
        8.0,
    )];
    assert!(AudienceShift::fit(&unvoted, FieldWeights::default()).is_none());
}

#[test]
fn votes_move_the_score_from_unreleased_to_released() {
    assert_eq!(audience_share(0), 0.0);
    assert!((audience_share(AUDIENCE_VOTES_HALF as u32) - 0.5).abs() < 1e-6);
    assert!(audience_share(100_000) > 0.99);

    assert_eq!(blend(6.0, None, 5000), (6.0, 0.0));
    let (rating, share) = blend(6.0, Some(8.0), 100);
    assert!((share - 0.5).abs() < 1e-6);
    assert!((rating - 7.0).abs() < 1e-5);
}

#[test]
fn track_records_nudge_the_similarity_rating() {
    // Cast and crew a point above typical add a point.
    assert!((nudged(6.0, Some(7.5), 6.5) - 7.0).abs() < 1e-5);
    assert!((nudged(6.0, None, 6.5) - 6.0).abs() < 1e-6);
    assert_eq!(nudged(9.5, Some(9.0), 6.5), 10.0);
}

#[test]
fn released_films_use_the_audience_and_unreleased_ones_similarity() {
    let rated_movies = director_fan();
    let upcoming = movie(
        "Upcoming",
        Spec {
            id: 60,
            genres: vec![HORROR],
            directors: vec![7],
            ..Spec::default()
        },
    );
    let released = voted(
        movie(
            "Released",
            Spec {
                id: 61,
                genres: vec![HORROR],
                directors: vec![7],
                ..Spec::default()
            },
        ),
        5.0,
        100_000,
    );
    let candidates = [upcoming.clone(), released.clone()];
    let p = profile(&rated_movies, &candidates, &[]);
    let signals = Signals {
        shift: AudienceShift::fit(&rated_movies, FieldWeights::default()),
        track_records: Some(TrackRecords {
            typical: 6.5,
            estimates: HashMap::from([(upcoming.id, 7.0)]),
        }),
    };

    let up = scoring::explain_with(&p, &signals, &upcoming).breakdown;
    assert!(up.audience.is_none());
    assert_eq!(up.audience_share, 0.0);
    let nudge = up.track_record.as_ref().expect("estimate given").nudge;
    assert!((nudge - 0.5).abs() < 1e-5);
    assert!((up.rating - (up.similarity + 0.5)).abs() < 1e-5, "{up:?}");

    let out = scoring::explain_with(&p, &signals, &released).breakdown;
    let audience = out.audience.as_ref().expect("votes and a shift");
    assert!(out.audience_share > 0.99);
    assert!((audience.rating - (5.0 + audience.shift.total())).abs() < 1e-5);
    assert!((out.rating - audience.rating).abs() < 0.05, "{out:?}");
    // The user rates director 7 above the crowd, so above the 5.0 audience.
    assert!(audience.rating > 6.0, "{audience:?}");

    // Without signals, both are scored by similarity alone, as before.
    let plain = scoring::rank(&p, &candidates);
    let with_nothing = scoring::rank_with(&p, &Signals::default(), &candidates);
    for (a, b) in plain.iter().zip(&with_nothing) {
        assert_eq!(a.score, b.score);
        assert_eq!(a.breakdown.rating, a.breakdown.similarity);
    }
}
