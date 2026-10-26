//! Builders for test movies and profiles, so tests never need the network.

use movie_match::features::{self, FieldWeights};
use movie_match::profile::{RatedDetails, TasteProfile};
use movie_match::tfidf::Idf;
use movie_match::tmdb::models::{CastMember, Credits, CrewMember, Genre, Keyword, Keywords};
use movie_match::tmdb::MovieDetails;

pub const DRAMA: u32 = 18;
pub const COMEDY: u32 = 35;
pub const HORROR: u32 = 27;
pub const ROMANCE: u32 = 10749;

/// The parts of a movie a test cares about; everything else is left empty.
/// Names are derived from ids: director 7 is "Director 7", keyword 500 is "kw500".
#[derive(Default)]
pub struct Spec {
    pub id: u64,
    pub genres: Vec<u32>,
    pub keywords: Vec<u64>,
    pub directors: Vec<u64>,
    /// Actor ids in billing order.
    pub cast: Vec<u64>,
}

pub fn movie(title: &str, spec: Spec) -> MovieDetails {
    MovieDetails {
        id: spec.id,
        title: title.to_string(),
        release_date: String::new(),
        genres: spec
            .genres
            .into_iter()
            .map(|id| Genre {
                id,
                name: format!("Genre {id}"),
            })
            .collect(),
        keywords: Keywords {
            keywords: spec
                .keywords
                .into_iter()
                .map(|id| Keyword {
                    id,
                    name: format!("kw{id}"),
                })
                .collect(),
        },
        credits: Credits {
            cast: spec
                .cast
                .into_iter()
                .zip(0..)
                .map(|(id, order)| CastMember {
                    id,
                    name: format!("Actor {id}"),
                    order,
                })
                .collect(),
            crew: spec
                .directors
                .into_iter()
                .map(|id| CrewMember {
                    id,
                    name: format!("Director {id}"),
                    job: "Director".to_string(),
                })
                .collect(),
        },
    }
}

pub fn rated(movie: &MovieDetails, rating: f32) -> RatedDetails {
    RatedDetails {
        movie: movie.clone(),
        rating,
    }
}

/// `count` dramas that share nothing else, to make the drama genre common.
pub fn filler_dramas(count: u64) -> Vec<MovieDetails> {
    (0..count)
        .map(|i| {
            movie(
                "Filler",
                Spec {
                    id: 1000 + i,
                    genres: vec![DRAMA],
                    directors: vec![1000 + i],
                    ..Spec::default()
                },
            )
        })
        .collect()
}

/// A profile whose IDF comes from the rated movies, `candidates` and `extra`.
pub fn profile(
    rated: &[RatedDetails],
    candidates: &[MovieDetails],
    extra: &[MovieDetails],
) -> TasteProfile {
    let weights = FieldWeights::default();
    let documents: Vec<_> = rated
        .iter()
        .map(|r| &r.movie)
        .chain(candidates)
        .chain(extra)
        .map(|m| features::extract(m, &weights))
        .collect();
    TasteProfile::new(rated, Idf::from_documents(&documents), weights)
}

/// Every movie rated 10/10.
pub fn favourites(movies: &[MovieDetails]) -> Vec<RatedDetails> {
    movies.iter().map(|m| rated(m, 10.0)).collect()
}
