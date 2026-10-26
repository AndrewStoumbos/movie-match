use movie_match::profile::{parse_rated_movies, UNRATED};

#[test]
fn parses_titles_ids_and_ratings() {
    let movies = parse_rated_movies(
        r#"
        [[movies]]
        title = "Arrival"
        year = 2016

        [[movies]]
        tmdb_id = 546554
        rating = 7.5
        "#,
    )
    .expect("valid file");

    assert_eq!(movies.len(), 2);
    assert_eq!(movies[0].title.as_deref(), Some("Arrival"));
    assert_eq!(movies[0].year, Some(2016));
    assert_eq!(movies[0].rating(), UNRATED);
    assert_eq!(movies[1].tmdb_id, Some(546554));
    assert_eq!(movies[1].rating(), 7.5);
    assert_eq!(movies[1].label(), "TMDB movie 546554");
}

#[test]
fn rejects_entries_without_title_or_id() {
    let err = parse_rated_movies("[[movies]]\nrating = 8\n").unwrap_err();
    assert!(err
        .to_string()
        .contains("movie #1 needs a title or a tmdb_id"));

    let blank = parse_rated_movies("[[movies]]\ntitle = \"  \"\n");
    assert!(blank.is_err());
}

#[test]
fn rejects_ratings_outside_half_to_ten() {
    for rating in ["0", "10.5", "-1"] {
        let text = format!("[[movies]]\ntitle = \"X\"\nrating = {rating}\n");
        let err = parse_rated_movies(&text).unwrap_err();
        assert!(
            err.to_string().contains("ratings go from 0.5 to 10"),
            "rating {rating}: {err}"
        );
    }
}
