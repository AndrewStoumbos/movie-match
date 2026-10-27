use movie_match::features::{self, FieldWeights};
use movie_match::tfidf::{Idf, Vector};
use movie_match::tmdb::MovieDetails;

fn fixture() -> MovieDetails {
    let json = include_str!("fixtures/movie_details.json");
    serde_json::from_str(json).expect("fixture is valid TMDB JSON")
}

#[test]
fn parses_details_with_keywords_and_credits() {
    let movie = fixture();
    assert_eq!(movie.title, "Fixture Movie");
    assert_eq!(movie.genres.len(), 2);
    assert_eq!(movie.keywords.keywords.len(), 2);
    assert_eq!(movie.credits.cast.len(), 6);
    let directors: Vec<_> = movie.crew_with_job("Director").map(|c| c.id).collect();
    assert_eq!(directors, [201]);
}

#[test]
fn extracts_weighted_tokens_per_field() {
    let weights = FieldWeights::default();
    let tokens = features::extract(&fixture(), &weights);

    assert_eq!(tokens["genre:878"], weights.genre);
    assert_eq!(tokens["kw:4379"], weights.keyword);
    assert_eq!(tokens["dir:201"], weights.director);
    // Credited twice (Screenplay and Writer) but counted once.
    assert_eq!(tokens["writer:202"], weights.writer);
    assert!(
        !tokens.keys().any(|t| t.ends_with(":203")),
        "composer ignored"
    );
}

#[test]
fn cast_weight_follows_billing_order_and_limit() {
    let weights = FieldWeights::default();
    let tokens = features::extract(&fixture(), &weights);

    assert_eq!(tokens["cast:101"], weights.cast);
    assert_eq!(tokens["cast:102"], weights.cast / 2.0);
    assert_eq!(tokens["cast:103"], weights.cast / 3.0);
    assert!(tokens.contains_key("cast:105"));
    assert!(!tokens.contains_key("cast:106"), "only the top 5 are used");
}

#[test]
fn idf_favours_rare_tokens_and_never_drops_below_one() {
    let common = features::Features::from([("genre:18".to_string(), 1.0)]);
    let both = features::Features::from([("genre:18".to_string(), 1.0), ("kw:1".to_string(), 1.0)]);
    let idf = Idf::from_documents([&common, &common, &common, &both]);

    assert!(idf.weight("kw:1") > idf.weight("genre:18"));
    assert!(idf.weight("unseen") > idf.weight("kw:1"));
    assert!((idf.weight("genre:18") - 1.0).abs() < 1e-6);
}

#[test]
fn vectors_are_unit_length_and_empty_features_give_empty_vectors() {
    let idf = Idf::default();
    let tokens = features::extract(&fixture(), &FieldWeights::default());
    let vector = Vector::new(&tokens, &idf);
    assert!((vector.cosine(&vector) - 1.0).abs() < 1e-5);

    let empty = Vector::new(&features::Features::new(), &idf);
    assert!(empty.is_empty());
    assert_eq!(empty.cosine(&vector), 0.0);
}

#[test]
fn labels_name_each_token() {
    let labels = features::labels(&fixture(), &FieldWeights::default());

    assert_eq!(labels["genre:878"], "genre Science Fiction");
    assert_eq!(labels["kw:4379"], "keyword \"time travel\"");
    assert_eq!(labels["dir:201"], "director The Director");
    assert_eq!(labels["writer:202"], "writer The Writer");
    assert_eq!(labels["cast:101"], "actor Lead Actor");
}

#[test]
fn tone_keywords_count_less_than_content_keywords() {
    let mut movie = fixture();
    movie
        .keywords
        .keywords
        .push(movie_match::tmdb::models::Keyword {
            id: 325815,
            name: "Farcical".to_string(),
        });
    let weights = FieldWeights::default();
    let tokens = features::extract(&movie, &weights);
    let labels = features::labels(&movie, &weights);

    assert_eq!(tokens["kw:325815"], weights.tone);
    assert_eq!(tokens["kw:4379"], weights.keyword);
    assert!(weights.tone < weights.keyword);
    assert_eq!(labels["kw:325815"], "tone \"Farcical\"");
}

#[test]
fn trimming_unused_credits_keeps_features_and_labels() {
    let weights = FieldWeights::default();
    let full = fixture();
    let mut trimmed = full.clone();
    features::trim_to_used(&mut trimmed, &weights);

    assert_eq!(trimmed.credits.cast.len(), weights.cast_limit);
    assert!(trimmed
        .credits
        .crew
        .iter()
        .all(|c| c.job == "Director" || c.job == "Screenplay" || c.job == "Writer"));
    assert_eq!(
        features::extract(&trimmed, &weights),
        features::extract(&full, &weights)
    );
    assert_eq!(
        features::labels(&trimmed, &weights),
        features::labels(&full, &weights)
    );
}
