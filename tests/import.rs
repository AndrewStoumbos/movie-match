use movie_match::import::parse_letterboxd_csv;

const RATINGS_CSV: &str = "\u{feff}Date,Name,Year,Letterboxd URI,Rating
2026-05-21,The Dark Knight,2008,https://boxd.it/2b0k,5
2026-05-22,\"Crouching Tiger, Hidden Dragon\",2000,https://boxd.it/abcd,3.5
2026-05-23,Some Flop,,https://boxd.it/efgh,0.5
";

#[test]
fn parses_letterboxd_ratings_and_doubles_stars() {
    let movies = parse_letterboxd_csv(RATINGS_CSV).expect("valid export");

    assert_eq!(movies.len(), 3);
    assert_eq!(movies[0].title.as_deref(), Some("The Dark Knight"));
    assert_eq!(movies[0].year, Some(2008));
    assert_eq!(movies[0].rating, Some(10.0));
    assert_eq!(movies[0].tmdb_id, None);
    assert_eq!(
        movies[1].title.as_deref(),
        Some("Crouching Tiger, Hidden Dragon")
    );
    assert_eq!(movies[1].rating, Some(7.0));
    assert_eq!(movies[2].year, None);
    assert_eq!(movies[2].rating, Some(1.0));
}

#[test]
fn skips_rows_without_a_rating() {
    let csv = "Date,Name,Year,Letterboxd URI,Rating\n2026-01-01,Unrated,2020,uri,\n";
    assert!(parse_letterboxd_csv(csv).expect("valid").is_empty());
}

#[test]
fn rejects_files_that_are_not_ratings_exports() {
    let watched = "Date,Name,Year,Letterboxd URI\n2026-01-01,Film,2020,uri\n";
    let err = parse_letterboxd_csv(watched).unwrap_err();
    assert!(err.to_string().contains("ratings.csv"), "{err}");
}

#[test]
fn rejects_bad_ratings_with_the_row_number() {
    let csv = "Date,Name,Year,Letterboxd URI,Rating\n2026-01-01,Film,2020,uri,7\n";
    let err = parse_letterboxd_csv(csv).unwrap_err();
    assert!(err.to_string().contains("row 2 (Film)"), "{err}");
}
