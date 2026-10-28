mod common;

use std::collections::HashMap;

use common::{movie, Spec};
use movie_match::features::FieldWeights;
use movie_match::quality::{
    credits, estimate, estimate_with, evaluate_track_record, records, EstimateSettings, Role,
    MIN_TRUTH_VOTES,
};
use movie_match::tmdb::{CreditedFilm, MovieDetails, PersonCredits};

fn film(id: u64, date: &str, score: f32, votes: u32) -> CreditedFilm {
    CreditedFilm {
        id,
        release_date: Some(date.to_string()),
        vote_average: Some(score),
        vote_count: Some(votes),
    }
}

/// A film by director `director`, released on `date`.
fn directed(id: u64, director: u64, date: &str) -> MovieDetails {
    let mut m = movie(
        "Film",
        Spec {
            id,
            directors: vec![director],
            ..Spec::default()
        },
    );
    m.release_date = date.to_string();
    m
}

#[test]
fn person_credits_keep_each_dated_voted_film_once() {
    let json = r#"{
        "cast": [
            {"id": 1, "title": "A", "release_date": "2010-01-01", "vote_average": 7.0, "vote_count": 10},
            {"id": 2, "title": "No date", "release_date": "", "vote_average": 6.0, "vote_count": 10},
            {"id": 3, "title": "No votes", "release_date": "2011-01-01", "vote_average": 0.0, "vote_count": 0}
        ],
        "crew": [
            {"id": 1, "title": "A again", "job": "Director", "release_date": "2010-01-01", "vote_average": 7.0, "vote_count": 10},
            {"id": 4, "title": "B", "job": "Writer", "release_date": null, "vote_average": 5.0, "vote_count": 3}
        ]
    }"#;
    let credits: PersonCredits = serde_json::from_str(json).expect("valid credits");
    let films = credits.films();
    assert_eq!(films.iter().map(|f| f.id).collect::<Vec<_>>(), [1]);
}

#[test]
fn only_films_made_before_count_and_never_the_film_itself() {
    let target = directed(10, 7, "2015-06-01");
    let filmographies = HashMap::from([(
        7,
        vec![
            film(1, "2010-01-01", 8.0, 100_000),
            // Released later, or the film itself: must be ignored.
            film(2, "2016-01-01", 2.0, 100_000),
            film(10, "2015-06-01", 2.0, 100_000),
        ],
    )]);
    let used = credits(&target, &FieldWeights::default(), 3);
    // Explicit cautious settings, so the arithmetic doesn't depend on the defaults.
    let settings = EstimateSettings {
        prior_votes: 100.0,
        person_shrink: 3.0,
        estimate_shrink: 2.0,
    };
    let found =
        estimate_with(&target, &used, &filmographies, 6.0, &settings).expect("an earlier film");
    // One earlier film at ~8.0: the director counts 3 × 1/(1 + 3) = 0.75 against
    // the typical score's 2, so (0.75 × 8 + 2 × 6) / 2.75.
    let expected = (0.75 * 8.0 + 2.0 * 6.0) / 2.75;
    assert!((found - expected).abs() < 0.01, "{found} vs {expected}");
}

#[test]
fn by_default_a_well_voted_track_record_is_trusted_fully() {
    let target = directed(10, 7, "2015-06-01");
    let filmographies = HashMap::from([(7, vec![film(1, "2010-01-01", 8.0, 100_000)])]);
    let used = credits(&target, &FieldWeights::default(), 3);
    let found = estimate(&target, &used, &filmographies, 6.0).expect("an earlier film");
    // No pull toward the typical score beyond 30 votes' worth on 100,000.
    assert!((found - 8.0).abs() < 0.01, "{found}");
}

#[test]
fn films_with_few_votes_barely_move_a_record() {
    let target = directed(10, 7, "2020-01-01");
    let few = HashMap::from([(7, vec![film(1, "2010-01-01", 10.0, 1)])]);
    let many = HashMap::from([(7, vec![film(1, "2010-01-01", 10.0, 100_000)])]);
    let used = credits(&target, &FieldWeights::default(), 3);
    let from_few = estimate(&target, &used, &few, 6.0).expect("estimate");
    let from_many = estimate(&target, &used, &many, 6.0).expect("estimate");
    assert!(
        from_few < 6.0 + (from_many - 6.0) * 0.05,
        "{from_few} vs {from_many}"
    );
}

#[test]
fn no_release_date_or_no_earlier_films_means_no_estimate() {
    let filmographies = HashMap::from([(7, vec![film(1, "2030-01-01", 8.0, 1000)])]);
    let later_only = directed(10, 7, "2020-01-01");
    let used = credits(&later_only, &FieldWeights::default(), 3);
    assert_eq!(estimate(&later_only, &used, &filmographies, 6.0), None);

    let undated = directed(10, 7, "");
    assert_eq!(estimate(&undated, &used, &filmographies, 6.0), None);
}

#[test]
fn credits_list_each_role_once_with_its_weight() {
    let weights = FieldWeights::default();
    let mut m = movie(
        "Film",
        Spec {
            id: 1,
            directors: vec![7],
            cast: vec![20, 21, 22, 23],
            ..Spec::default()
        },
    );
    m.credits.crew.push(movie_match::tmdb::models::CrewMember {
        id: 7,
        name: "Director 7".into(),
        job: "Screenplay".into(),
    });
    let list = credits(&m, &weights, 3);

    assert!(list.contains(&movie_match::quality::Credit {
        person: 7,
        name: "Director 7".into(),
        role: Role::Director,
        weight: weights.director
    }));
    assert!(list.iter().any(|c| c.person == 7 && c.role == Role::Writer));
    let cast: Vec<_> = list.iter().filter(|c| c.role == Role::Cast).collect();
    assert_eq!(cast.len(), 3);
    assert_eq!(cast[1].weight, weights.cast / 2.0);
}

#[test]
fn track_records_separate_good_films_from_bad_ones() {
    // Director 1 has made great films, director 2 poor ones; each has a new film.
    let mut good = directed(100, 1, "2020-01-01");
    good.vote_average = Some(8.0);
    good.vote_count = Some(MIN_TRUTH_VOTES);
    let mut bad = directed(200, 2, "2020-01-01");
    bad.vote_average = Some(4.0);
    bad.vote_count = Some(MIN_TRUTH_VOTES);
    let filmographies = HashMap::from([
        (
            1,
            (0..5)
                .map(|i| film(10 + i, "2010-01-01", 8.5, 5000))
                .collect(),
        ),
        (
            2,
            (0..5)
                .map(|i| film(20 + i, "2010-01-01", 4.5, 5000))
                .collect(),
        ),
    ]);

    let report = evaluate_track_record(&[good, bad], &filmographies, &FieldWeights::default(), 3);
    assert_eq!(report.films, 2);
    let [typical, directors, writers, ..] = report.results.as_slice() else {
        panic!("one result per method");
    };
    assert_eq!(typical.coverage, 0.0);
    assert_eq!(typical.spearman, None);
    assert_eq!(directors.coverage, 1.0);
    assert!(directors.mae < typical.mae, "{directors:?} vs {typical:?}");
    assert_eq!(directors.spearman, Some(1.0));
    // No writers credited: nothing to go on.
    assert_eq!(writers.coverage, 0.0);
}

#[test]
fn default_settings_reproduce_estimate() {
    let target = directed(10, 7, "2015-06-01");
    let filmographies = HashMap::from([(7, vec![film(1, "2010-01-01", 8.0, 500)])]);
    let used = credits(&target, &FieldWeights::default(), 3);
    assert_eq!(
        movie_match::quality::estimate_with(
            &target,
            &used,
            &filmographies,
            6.0,
            &movie_match::quality::EstimateSettings::default()
        ),
        estimate(&target, &used, &filmographies, 6.0)
    );
}

#[test]
fn tuning_splits_films_and_keeps_check_half_unseen() {
    // Ten good-director films and ten poor-director ones, scored true to form.
    let mut films = Vec::new();
    let mut filmographies = HashMap::new();
    for i in 0..20u64 {
        let director = 1000 + i;
        let good = i % 2 == 0;
        let mut m = directed(100 + i, director, "2020-01-01");
        m.vote_average = Some(if good { 8.0 } else { 4.0 });
        m.vote_count = Some(MIN_TRUTH_VOTES);
        films.push(m);
        let past = if good { 8.0 } else { 4.0 };
        filmographies.insert(
            director,
            (0..4)
                .map(|k| film(10_000 + i * 10 + k, "2010-01-01", past, 50_000))
                .collect(),
        );
    }

    let report = movie_match::quality::tune(&films, &filmographies, &FieldWeights::default(), 3);
    assert_eq!(report.tune_films + report.check_films, 20);
    assert_eq!(report.tune_films, 10);
    let [(typical_name, typical), (_, defaults), (tuned_name, tuned)] = report.check;
    assert_eq!(typical_name, "typical score");
    assert_eq!(tuned_name, "tuned settings");
    // Track records are perfectly informative here, so tuning should trust them
    // more than the cautious defaults, and both beat the typical score.
    assert!(tuned.mae <= defaults.mae, "{tuned:?} vs {defaults:?}");
    assert!(defaults.mae < typical.mae);
    assert_eq!(report.best.estimate_shrink, 0.0);
}

#[test]
fn records_name_each_person_and_add_up_to_the_estimate() {
    // Director 7 (weight 3) averages 8 over two films; actor 9 (weight 2) has 5.
    let mut target = directed(10, 7, "2015-06-01");
    target
        .credits
        .cast
        .push(movie_match::tmdb::models::CastMember {
            id: 9,
            name: "Actor 9".into(),
            order: 0,
        });
    let filmographies = HashMap::from([
        (
            7,
            vec![
                film(1, "2010-01-01", 8.0, 100_000),
                film(2, "2012-01-01", 8.0, 100_000),
            ],
        ),
        (9, vec![film(3, "2011-01-01", 5.0, 100_000)]),
    ]);
    let used = credits(&target, &FieldWeights::default(), 3);

    let found = records(&target, &used, &filmographies, 6.0);
    let summary: Vec<_> = found
        .iter()
        .map(|r| (r.name.as_str(), r.role, r.films))
        .collect();
    assert_eq!(
        summary,
        [
            ("Director 7", Role::Director, 2),
            ("Actor 9", Role::Cast, 1)
        ]
    );
    let total: f32 = found.iter().map(|r| r.weight).sum();
    let weighted: f32 = found.iter().map(|r| r.weight * r.record).sum();
    let estimated = estimate(&target, &used, &filmographies, 6.0).expect("records");
    assert!((weighted / total - estimated).abs() < 1e-5);
    // The director pulls up by 3 × 2, the actor down by 2 × 1.
    assert!(
        found[0].pull(6.0) > 5.9 && found[1].pull(6.0) < -1.9,
        "{found:?}"
    );
}
