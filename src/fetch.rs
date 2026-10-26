//! TMDB access with an optional disk cache in front of movie details.

use std::time::Duration;

use anyhow::{Context, Result};
use futures::stream::{self, StreamExt, TryStreamExt};

use crate::cache::DiskCache;
use crate::tmdb::{self, MovieDetails, TmdbClient};

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
        // Owned ids keep the futures `Send` (see `recommend::resolve`).
        stream::iter(ids.to_vec())
            .map(|id| self.one_details(id))
            .buffered(CONCURRENT_REQUESTS)
            .try_collect()
            .await
    }

    /// Details for each id TMDB still has, in the same order. A movie TMDB answers
    /// 404 for (say it has since removed it) is skipped, with one summary warning on
    /// stderr, so a stale id in a cached list doesn't fail every run. Any other
    /// error, such as a timeout, rate limit or server error, fails the call, so a
    /// network blip can't quietly drop movies. Also fails if none of the ids load.
    pub async fn available_details(&self, ids: &[u64]) -> Result<Vec<MovieDetails>> {
        let results = self.details_or_not_found(ids).await?;
        let mut movies = Vec::with_capacity(results.len());
        let mut skipped = Vec::new();
        let mut first_error = None;
        for (&id, result) in ids.iter().zip(results) {
            match result {
                Ok(movie) => movies.push(movie),
                Err(err) => {
                    skipped.push(id.to_string());
                    first_error.get_or_insert(err);
                }
            }
        }
        if movies.is_empty() {
            if let Some(err) = first_error {
                return Err(err);
            }
        }
        if !skipped.is_empty() {
            eprintln!(
                "warning: skipped {} of {} movies that TMDB no longer has (ids {})",
                skipped.len(),
                ids.len(),
                skipped.join(", ")
            );
        }
        Ok(movies)
    }

    /// Details for each id, in the same order, with `None` for ids TMDB answers 404
    /// for (removed, or never a movie). Any other error fails the call, like
    /// [`Self::available_details`].
    pub async fn details_if_found(&self, ids: &[u64]) -> Result<Vec<Option<MovieDetails>>> {
        let results = self.details_or_not_found(ids).await?;
        Ok(results.into_iter().map(Result::ok).collect())
    }

    /// Each id's details, in order. The outer `Result` is a real failure; an inner
    /// `Err` is a movie TMDB doesn't have (a 404).
    async fn details_or_not_found(&self, ids: &[u64]) -> Result<Vec<Result<MovieDetails>>> {
        // Owned ids keep the futures `Send` (see `recommend::resolve`).
        stream::iter(ids.to_vec())
            .map(|id| async move {
                match self.one_details(id).await {
                    Ok(movie) => Ok(Ok(movie)),
                    Err(err) if tmdb::is_not_found(&err) => Ok(Err(err)),
                    Err(err) => Err(err),
                }
            })
            .buffered(CONCURRENT_REQUESTS)
            .try_collect()
            .await
    }

    async fn one_details(&self, id: u64) -> Result<MovieDetails> {
        let key = details_cache_key(id);
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

/// The cache key for one movie's details.
pub fn details_cache_key(id: u64) -> String {
    // v2: details now keep the overview and poster; older entries lack them.
    format!("details-v2/{id}")
}
