//! Fetches everything needed from TMDB and ranks upcoming releases.

use std::collections::HashSet;

use anyhow::{Context, Result};
use futures::stream::{self, StreamExt, TryStreamExt};

use crate::features::{self, FieldWeights};
use crate::profile::{LikedMovie, TasteProfile};
use crate::scoring;
use crate::tfidf::Idf;
use crate::tmdb::{Movie, MovieDetails, Page, TmdbClient};

/// How many TMDB requests to run at once. Well under TMDB's rate limit.
const CONCURRENT_REQUESTS: usize = 8;

#[derive(Debug, Clone)]
pub struct Options {
    /// Pages of `/movie/upcoming` to rate (20 movies per page).
    pub upcoming_pages: u32,
    /// Pages of `/movie/popular` added to the IDF reference set.
    pub reference_pages: u32,
    pub weights: FieldWeights,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            upcoming_pages: 2,
            reference_pages: 5,
            weights: FieldWeights::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Ranked {
    pub movie: MovieDetails,
    pub score: u8,
    pub closest_liked: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Ranking {
    /// The TMDB movies the liked-movies file resolved to.
    pub liked: Vec<MovieDetails>,
    /// Liked titles TMDB search found no match for.
    pub unmatched: Vec<String>,
    /// Upcoming releases, best match first.
    pub ranked: Vec<Ranked>,
}

pub async fn rank_upcoming(
    client: &TmdbClient,
    liked: &[LikedMovie],
    options: &Options,
) -> Result<Ranking> {
    // Every id is fetched once, even if it shows up in several lists.
    let mut seen = HashSet::new();
    let mut liked_ids = Vec::new();
    let mut unmatched = Vec::new();
    let searches: Vec<_> = stream::iter(liked)
        .map(|m| async move { (m, client.search_movie(&m.title, m.year).await) })
        .buffered(CONCURRENT_REQUESTS)
        .collect()
        .await;
    for (wanted, found) in searches {
        let found = found.with_context(|| format!("searching TMDB for \"{}\"", wanted.title))?;
        match found {
            Some(movie) => {
                if seen.insert(movie.id) {
                    liked_ids.push(movie.id);
                }
            }
            None => unmatched.push(wanted.title.clone()),
        }
    }

    let upcoming_ids = list_ids(options.upcoming_pages, &mut seen, |p| client.upcoming(p)).await?;
    let reference_ids = list_ids(options.reference_pages, &mut seen, |p| client.popular(p)).await?;

    let liked = fetch_details(client, &liked_ids).await?;
    let upcoming = fetch_details(client, &upcoming_ids).await?;
    let reference = fetch_details(client, &reference_ids).await?;

    let documents: Vec<_> = liked
        .iter()
        .chain(&upcoming)
        .chain(&reference)
        .map(|movie| features::extract(movie, &options.weights))
        .collect();
    let profile = TasteProfile::new(&liked, Idf::from_documents(&documents), options.weights);

    let mut ranked: Vec<Ranked> = upcoming
        .into_iter()
        .map(|movie| {
            let found = scoring::score_detailed(&profile, &movie);
            Ranked {
                score: found.score,
                closest_liked: found.closest_liked.map(str::to_owned),
                movie,
            }
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.movie.title.cmp(&b.movie.title))
    });

    Ok(Ranking {
        liked,
        unmatched,
        ranked,
    })
}

/// Ids from the first `pages` pages of a TMDB list, skipping any already in `seen`.
async fn list_ids<F, Fut>(pages: u32, seen: &mut HashSet<u64>, fetch_page: F) -> Result<Vec<u64>>
where
    F: Fn(u32) -> Fut,
    Fut: std::future::Future<Output = Result<Page<Movie>>>,
{
    let mut ids = Vec::new();
    for page in 1..=pages {
        let page = fetch_page(page).await?;
        let last_page = page.page >= page.total_pages;
        ids.extend(
            page.results
                .iter()
                .map(|m| m.id)
                .filter(|id| seen.insert(*id)),
        );
        if last_page {
            break;
        }
    }
    Ok(ids)
}

async fn fetch_details(client: &TmdbClient, ids: &[u64]) -> Result<Vec<MovieDetails>> {
    stream::iter(ids)
        .map(|&id| async move {
            client
                .movie_details(id)
                .await
                .with_context(|| format!("fetching details for TMDB movie {id}"))
        })
        .buffered(CONCURRENT_REQUESTS)
        .try_collect()
        .await
}
