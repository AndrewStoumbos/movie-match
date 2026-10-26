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
