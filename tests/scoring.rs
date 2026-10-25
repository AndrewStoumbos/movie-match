use movie_match::profile::TasteProfile;
use movie_match::scoring::score;
use movie_match::tmdb::Movie;

const SCI_FI: u32 = 878;
const DRAMA: u32 = 18;
const COMEDY: u32 = 35;

fn movie(genre_ids: Vec<u32>) -> Movie {
    Movie {
        id: 0,
        title: String::new(),
        overview: String::new(),
        release_date: String::new(),
        genre_ids,
        vote_average: 0.0,
    }
}

#[test]
fn matching_genres_score_higher() {
    let liked = [movie(vec![SCI_FI, DRAMA]), movie(vec![SCI_FI])];
    let profile = TasteProfile::from_movies(&liked);

    let sci_fi = score(&profile, &movie(vec![SCI_FI]));
    let comedy = score(&profile, &movie(vec![COMEDY]));

    assert_eq!(sci_fi, 100);
    assert_eq!(comedy, 0);
}

#[test]
fn movie_without_genres_scores_zero() {
    let profile = TasteProfile::from_movies(&[movie(vec![SCI_FI])]);
    assert_eq!(score(&profile, &movie(vec![])), 0);
}
