use movie_match::movielens::{parse_links, recent_ratings};

const LINKS: &str = "movieId,imdbId,tmdbId
1,0114709,862
2,0113497,8844
3,0113228,
4,0114885,8844
";

#[test]
fn links_map_movielens_ids_to_tmdb_and_skip_missing_ones() {
    let links = parse_links(LINKS).expect("valid links");
    assert_eq!(links.len(), 3);
    assert_eq!(links[&1], 862);
    assert!(!links.contains_key(&3));
}

#[test]
fn keeps_each_users_most_recent_linked_ratings_doubled() {
    let links = parse_links(LINKS).expect("valid links");
    let ratings = "userId,movieId,rating,timestamp
1,1,4.0,100
1,2,2.5,300
1,3,5.0,400
2,1,0.5,50
";
    let users = recent_ratings(ratings, &links, 10).expect("valid ratings");

    assert_eq!(users.len(), 2);
    // Movie 3 has no TMDB id; newest first; stars doubled.
    assert_eq!(users[&1], [(8844, 5.0), (862, 8.0)]);
    assert_eq!(users[&2], [(862, 1.0)]);
}

#[test]
fn caps_ratings_per_user_and_merges_movies_linked_twice() {
    let links = parse_links(LINKS).expect("valid links");
    // Movies 2 and 4 both link to TMDB 8844: the newer rating wins.
    let ratings = "userId,movieId,rating,timestamp
1,1,4.0,100
1,2,2.5,300
1,4,3.5,200
";
    let all = recent_ratings(ratings, &links, 10).expect("valid ratings");
    assert_eq!(all[&1], [(8844, 5.0), (862, 8.0)]);

    let capped = recent_ratings(ratings, &links, 1).expect("valid ratings");
    assert_eq!(capped[&1], [(8844, 5.0)]);
}

#[test]
fn rejects_ratings_outside_half_to_five_stars() {
    let links = parse_links(LINKS).expect("valid links");
    let err =
        recent_ratings("userId,movieId,rating,timestamp\n1,1,6,100\n", &links, 10).unwrap_err();
    assert!(err.to_string().contains("row 2"), "{err}");
}
