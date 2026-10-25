//! Builders for test movies, so tests never need the network.

use movie_match::tmdb::models::{CastMember, Credits, CrewMember, Genre, Keyword, Keywords};
use movie_match::tmdb::MovieDetails;

pub const DRAMA: u32 = 18;
pub const COMEDY: u32 = 35;
pub const HORROR: u32 = 27;
pub const ROMANCE: u32 = 10749;

/// The parts of a movie a test cares about; everything else is left empty.
#[derive(Default)]
pub struct Spec {
    pub genres: Vec<u32>,
    pub keywords: Vec<u64>,
    pub directors: Vec<u64>,
    /// Actor ids in billing order.
    pub cast: Vec<u64>,
}

pub fn movie(title: &str, spec: Spec) -> MovieDetails {
    MovieDetails {
        id: 0,
        title: title.to_string(),
        release_date: String::new(),
        genres: spec
            .genres
            .into_iter()
            .map(|id| Genre {
                id,
                name: String::new(),
            })
            .collect(),
        keywords: Keywords {
            keywords: spec
                .keywords
                .into_iter()
                .map(|id| Keyword {
                    id,
                    name: String::new(),
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
                    name: String::new(),
                    order,
                })
                .collect(),
            crew: spec
                .directors
                .into_iter()
                .map(|id| CrewMember {
                    id,
                    name: String::new(),
                    job: "Director".to_string(),
                })
                .collect(),
        },
    }
}

/// `count` dramas that share nothing else, to make the drama genre common.
pub fn filler_dramas(count: u64) -> Vec<MovieDetails> {
    (0..count)
        .map(|i| {
            movie(
                "Filler",
                Spec {
                    genres: vec![DRAMA],
                    directors: vec![1000 + i],
                    ..Spec::default()
                },
            )
        })
        .collect()
}
