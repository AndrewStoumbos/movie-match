mod common;

use common::{filler_dramas, movie, rated, Spec, COMEDY, HORROR};
use movie_match::evaluate::{leave_one_out, spearman, METHODS};
use movie_match::features::{self, FieldWeights};
use movie_match::profile::RatedDetails;
use movie_match::scoring::TrackRecords;
use movie_match::tfidf::Idf;

/// Where `name` sits in [`METHODS`].
fn method(name: &str) -> usize {
    METHODS
        .iter()
        .position(|&m| m == name)
        .unwrap_or_else(|| panic!("no method {name}"))
}

#[test]
fn spearman_measures_how_well_the_order_matches() {
    assert_eq!(spearman(&[1.0, 2.0, 3.0], &[10.0, 20.0, 30.0]), Some(1.0));
    assert_eq!(spearman(&[1.0, 2.0, 3.0], &[30.0, 20.0, 10.0]), Some(-1.0));
    // Ties share their average rank, so matching ties still correlate perfectly.
    assert_eq!(
        spearman(&[1.0, 2.0, 2.0, 3.0], &[5.0, 7.0, 7.0, 9.0]),
        Some(1.0)
    );
    // A constant can't order anything.
    assert_eq!(spearman(&[5.0, 5.0, 5.0], &[1.0, 2.0, 3.0]), None);
    assert_eq!(spearman(&[1.0], &[1.0]), None);
}

/// Pairs of near-identical movies the user rated alike: horrors 9, comedies 2.
fn paired_ratings() -> Vec<RatedDetails> {
    let film = |id: u64, genre: u32, director: u64, keyword: u64| {
        movie(
            "Film",
            Spec {
                id,
                genres: vec![genre],
                directors: vec![director],
                keywords: vec![keyword],
                ..Spec::default()
            },
        )
    };
    let mut rated_movies = Vec::new();
    for i in 0..3 {
        rated_movies.push(rated(&film(10 + i, HORROR, 100 + i, 200 + i), 9.0));
        rated_movies.push(rated(&film(20 + i, HORROR, 100 + i, 200 + i), 9.0));
        rated_movies.push(rated(&film(30 + i, COMEDY, 300 + i, 400 + i), 2.0));
        rated_movies.push(rated(&film(40 + i, COMEDY, 300 + i, 400 + i), 2.0));
    }
    rated_movies
}

fn idf_for(rated_movies: &[RatedDetails]) -> Idf {
    let weights = FieldWeights::default();
    let documents: Vec<_> = rated_movies
        .iter()
        .map(|r| &r.movie)
        .chain(&filler_dramas(10))
        .map(|m| features::extract(m, &weights))
        .collect();
    Idf::from_documents(&documents)
}

#[test]
fn similar_ratings_beat_the_average_when_taste_follows_similarity() {
    let rated_movies = paired_ratings();
    let evaluation = leave_one_out(
        &rated_movies,
        &idf_for(&rated_movies),
        FieldWeights::default(),
    );

    assert_eq!(evaluation.movies.len(), 12);
    assert_eq!(evaluation.results.len(), METHODS.len());
    let [movie_match, similarity, average, closest, tmdb, ..] = evaluation.results.as_slice()
    else {
        panic!("one result per method");
    };
    assert_eq!(movie_match.name, "movie-match");
    // Without audience votes or track records, movie-match is similarity alone.
    assert_eq!(similarity.name, "similarity only");
    assert_eq!(movie_match.mae, similarity.mae);
    // Every hidden movie has a twin rated the same, so following similar ratings
    // wins clearly over guessing the average.
    assert!(
        movie_match.mae < average.mae,
        "{movie_match:?} vs {average:?}"
    );
    assert_eq!(closest.mae, 0.0);
    assert!(movie_match.spearman.is_some_and(|s| s > 0.8));
    // The average is a constant guess, so it has no ranking score.
    assert_eq!(average.spearman, None);
    // None of these test movies has TMDB votes.
    assert_eq!(tmdb.count, 0);
}

#[test]
fn tmdb_baseline_only_counts_movies_with_votes() {
    let mut rated_movies = paired_ratings();
    rated_movies[0].movie.vote_average = Some(8.0);
    rated_movies[0].movie.vote_count = Some(1200);
    // An average with no votes behind it isn't a real score.
    rated_movies[1].movie.vote_average = Some(0.0);
    rated_movies[1].movie.vote_count = Some(0);

    let evaluation = leave_one_out(
        &rated_movies,
        &idf_for(&rated_movies),
        FieldWeights::default(),
    );
    let tmdb = &evaluation.results[method("TMDB audience score")];
    assert_eq!(tmdb.count, 1);
    // Rated 9, TMDB says 8.
    assert!((tmdb.mae - 1.0).abs() < 1e-6);
}

#[test]
fn fewer_than_two_ratings_predict_nothing() {
    let rated_movies = &paired_ratings()[..1];
    let evaluation = leave_one_out(
        rated_movies,
        &idf_for(rated_movies),
        FieldWeights::default(),
    );
    assert!(evaluation.movies.is_empty());
    assert!(evaluation.results.iter().all(|r| r.count == 0));
}

#[test]
fn a_steady_difference_from_the_audience_is_predicted_exactly() {
    // The user always rates one point above the TMDB audience.
    let mut rated_movies = paired_ratings();
    for r in &mut rated_movies {
        r.movie.vote_average = Some(r.rating - 1.0);
        r.movie.vote_count = Some(500);
    }

    let evaluation = leave_one_out(
        &rated_movies,
        &idf_for(&rated_movies),
        FieldWeights::default(),
    );
    let [movie_match, similarity, _, _, tmdb, rest @ ..] = evaluation.results.as_slice() else {
        panic!("one result per method");
    };
    // With 500 votes, movie-match is mostly the exact audience + shift prediction.
    assert!(
        movie_match.mae < similarity.mae,
        "{movie_match:?} vs {similarity:?}"
    );
    // Average diff, similar diff and all three shift strengths; then the
    // "est. quality" ones, which predict nothing without estimates.
    let (diffs, estimated) = rest.split_at(5);
    assert_eq!(diffs[0].name, "audience + avg diff");
    assert!((tmdb.mae - 1.0).abs() < 1e-5, "{tmdb:?}");
    for method in diffs {
        assert!(method.mae < 1e-4, "{method:?}");
        assert_eq!(method.count, rated_movies.len());
    }
    assert!(estimated.iter().all(|m| m.count == 0), "{estimated:?}");
}

#[test]
fn estimated_quality_matching_the_audience_gives_the_audience_results() {
    // The user rates one point above the audience; the estimates equal the real
    // audience scores, so each "est. quality" method matches its audience twin.
    let mut rated_movies = paired_ratings();
    for r in &mut rated_movies {
        r.movie.vote_average = Some(r.rating - 1.0);
        r.movie.vote_count = Some(500);
    }
    let estimates = TrackRecords {
        people: Default::default(),
        typical: 5.5,
        estimates: rated_movies
            .iter()
            .map(|r| (r.movie.id, r.rating - 1.0))
            .collect(),
    };

    let evaluation = movie_match::evaluate::leave_one_out_with_quality(
        &rated_movies,
        &idf_for(&rated_movies),
        FieldWeights::default(),
        Some(&estimates),
    );
    let mae = |name: &str| {
        evaluation
            .results
            .iter()
            .find(|r| r.name == name)
            .map(|r| (r.count, r.mae))
            .unwrap_or_else(|| panic!("no method {name}"))
    };
    for (estimated, audience) in [
        ("est. quality", "TMDB audience score"),
        ("est. quality + avg diff", "audience + avg diff"),
        ("est. quality + shift (light)", "audience + shift (light)"),
        ("est. quality + shift (medium)", "audience + shift (medium)"),
    ] {
        let (count, error) = mae(estimated);
        assert_eq!(count, rated_movies.len());
        assert!((error - mae(audience).1).abs() < 1e-5, "{estimated}");
    }
}

#[test]
fn per_feature_shifts_learn_genre_differences_an_average_misses() {
    // Every movie has an audience score of 6. The user rates the horrors 9 (+3) and
    // the comedies 2 (-4): an average difference of -0.5 fits neither.
    let mut rated_movies = paired_ratings();
    for r in &mut rated_movies {
        r.movie.vote_average = Some(6.0);
        r.movie.vote_count = Some(500);
    }

    let evaluation = leave_one_out(
        &rated_movies,
        &idf_for(&rated_movies),
        FieldWeights::default(),
    );
    let by_name = |name: &str| {
        evaluation
            .results
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("no method {name}"))
            .clone()
    };
    let avg_diff = by_name("audience + avg diff");
    let light = by_name("audience + shift (light)");
    // Hiding a horror leaves 5 at +3 and 6 at -4: average -9/11, a miss of 42/11.
    // Hiding a comedy mirrors it.
    assert!((avg_diff.mae - 42.0 / 11.0).abs() < 1e-4, "{avg_diff:?}");
    // Shifts learn most of the genre difference; the ridge holds some back.
    assert!(light.mae < avg_diff.mae / 3.0, "{light:?}");
    assert!(light.spearman.is_some_and(|s| s > 0.8), "{light:?}");
}

#[test]
fn across_users_groups_people_by_how_closely_they_follow_the_crowd() {
    // Same movies and ratings; one user agrees with the audience, one is its opposite.
    let with_audience = |flip: bool| {
        let mut rated_movies = paired_ratings();
        for r in &mut rated_movies {
            let audience = if flip { 11.0 - r.rating } else { r.rating };
            r.movie.vote_average = Some(audience);
            r.movie.vote_count = Some(500);
        }
        rated_movies
    };
    let users = [with_audience(false), with_audience(true)];
    let idf = idf_for(&users[0]);
    let evaluations =
        movie_match::evaluate::leave_one_out_each(&users, &idf, FieldWeights::default(), None);
    assert_eq!(evaluations.len(), 2);

    let report = movie_match::evaluate::across_users(&evaluations);
    assert_eq!(report.users, 2);
    assert_eq!(report.pooled[0].count, 24);
    assert!(report.wins.iter().sum::<f32>() <= 1.0 + 1e-6);
    let [crowd, contrarian] = report.taste_groups.as_slice() else {
        panic!("two users make two groups: {:?}", report.taste_groups);
    };
    assert!(crowd.agreement.0 > 0.9, "{crowd:?}");
    assert!(contrarian.agreement.1 < -0.9, "{contrarian:?}");
    // The raw audience score is spot on for the agreeing user, way off for the other.
    let audience = method("TMDB audience score");
    assert!(crowd.mae[audience] < 1e-5);
    assert!(contrarian.mae[audience] > 3.0);
}

#[test]
fn nudge_tuning_picks_a_strength_and_scores_it_on_unseen_users() {
    // Estimates that track the user's ratings perfectly, for four users: tuning
    // should nudge, and nudging should help on the users it didn't tune on.
    let users: Vec<_> = (0..4).map(|_| paired_ratings()).collect();
    let estimates = TrackRecords {
        people: Default::default(),
        typical: 5.5,
        estimates: users[0].iter().map(|r| (r.movie.id, r.rating)).collect(),
    };
    let idf = idf_for(&users[0]);
    let evaluations = movie_match::evaluate::leave_one_out_each(
        &users,
        &idf,
        FieldWeights::default(),
        Some(&estimates),
    );

    let report = movie_match::evaluate::tune_nudge(&evaluations, 5.5).expect("estimates given");
    assert_eq!((report.tune_users, report.check_users), (2, 2));
    assert_eq!(
        report.check.len(),
        movie_match::evaluate::NUDGE_STRENGTHS.len()
    );
    assert!(report.best > 0.0, "{report:?}");
    let at = |strength: f32| {
        report
            .check
            .iter()
            .find(|(s, _)| *s == strength)
            .map(|(_, scores)| *scores)
            .expect("strength scored")
    };
    assert!(at(report.best).mae < at(0.0).mae);
}

#[test]
fn nudge_tuning_needs_quality_estimates() {
    let users = [paired_ratings()];
    let evaluations = movie_match::evaluate::leave_one_out_each(
        &users,
        &idf_for(&users[0]),
        FieldWeights::default(),
        None,
    );
    assert!(movie_match::evaluate::tune_nudge(&evaluations, 6.0).is_none());
}
