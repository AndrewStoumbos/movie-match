//! TMDB access with an optional disk cache in front of movie details.

use std::time::Duration;

use anyhow::{Context, Result};
use futures::stream::{self, StreamExt, TryStreamExt};

use crate::cache::DiskCache;
use crate::tmdb::{MovieDetails, TmdbClient};

/// How many TMDB requests to run at once. Well under TMDB's rate limit.
pub const CONCURRENT_REQUESTS: usize = 8;

/// How long cached movie details are reused. Upcoming movies still gain cast and
/// keywords, so this is kept short.
pub const DETAILS_MAX_AGE: Duration = Duration::from_secs(3 * 24 * 60 * 60);

pub struct Fetcher {
    client: TmdbClient,
    cache: Option<DiskCache>,
}

impl Fetcher {
    pub fn new(client: TmdbClient, cache: Option<DiskCache>) -> Self {
        Self { client, cache }
    }

    pub fn client(&self) -> &TmdbClient {
        &self.client
    }

    pub fn cache(&self) -> Option<&DiskCache> {
        self.cache.as_ref()
    }

    /// Details for each id, in the same order. Cached details skip the network.
    pub async fn details(&self, ids: &[u64]) -> Result<Vec<MovieDetails>> {
        stream::iter(ids)
            .map(|&id| self.one_details(id))
            .buffered(CONCURRENT_REQUESTS)
            .try_collect()
            .await
    }

    /// Details for each id that loads, in the same order. A movie whose details
    /// fail to load (say TMDB has since removed it) is skipped with a warning on
    /// stderr, so one bad id in a cached list doesn't fail every run. Fails only if
    /// none of the ids load, which points at a wider problem like no network.
    pub async fn available_details(&self, ids: &[u64]) -> Result<Vec<MovieDetails>> {
        let results: Vec<Result<MovieDetails>> = stream::iter(ids)
            .map(|&id| self.one_details(id))
            .buffered(CONCURRENT_REQUESTS)
            .collect()
            .await;
        let mut movies = Vec::with_capacity(results.len());
        let mut errors = Vec::new();
        for result in results {
            match result {
                Ok(movie) => movies.push(movie),
                Err(err) => errors.push(err),
            }
        }
        if movies.is_empty() && !errors.is_empty() {
            return Err(errors.swap_remove(0));
        }
        for err in errors {
            eprintln!("warning: skipping movie: {err:#}");
        }
        Ok(movies)
    }

    async fn one_details(&self, id: u64) -> Result<MovieDetails> {
        let key = format!("details/{id}");
        if let Some(movie) = self.cache().and_then(|c| c.get(&key, DETAILS_MAX_AGE)) {
            return Ok(movie);
        }
        let movie = self
            .client
            .movie_details(id)
            .await
            .with_context(|| format!("fetching details for TMDB movie {id}"))?;
        if let Some(cache) = self.cache() {
            cache.put_or_warn(&key, &movie);
        }
        Ok(movie)
    }
}
