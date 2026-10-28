//! TMDB access with an optional disk cache in front of movie details.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::stream::{self, StreamExt, TryStreamExt};

use crate::cache::DiskCache;
use crate::tmdb::{self, CreditedFilm, MovieDetails, TmdbClient};

/// How many TMDB requests to run at once. Well under TMDB's rate limit.
pub const CONCURRENT_REQUESTS: usize = 8;

/// How long cached movie details are reused. Upcoming movies still gain cast and
/// keywords, so this is kept short.
pub const DETAILS_MAX_AGE: Duration = Duration::from_secs(3 * 24 * 60 * 60);

/// How long a person's cached filmography is reused. Only new releases change it.
pub const FILMOGRAPHY_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How many failed filmography requests [`Fetcher::filmographies_or_skip`] allows
/// before it stops asking TMDB.
pub const MAX_FILMOGRAPHY_FAILURES: usize = 20;

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

    /// Each person's films (see [`tmdb::PersonCredits::films`]), in the same order,
    /// with `None` for people TMDB answers 404 for. Only the trimmed list is cached.
    /// Any other error fails the call.
    pub async fn filmographies(&self, people: &[u64]) -> Result<Vec<Option<Vec<CreditedFilm>>>> {
        // Owned ids keep the futures `Send` (see `recommend::resolve`).
        stream::iter(people.to_vec())
            .map(|id| async move {
                match self.one_filmography(id).await {
                    Ok(films) => Ok(Some(films)),
                    Err(err) if tmdb::is_not_found(&err) => Ok(None),
                    Err(err) => Err(err),
                }
            })
            .buffered(CONCURRENT_REQUESTS)
            .try_collect()
            .await
    }

    /// Like [`Self::filmographies`], but a person whose films fail to load for any
    /// reason (a rate limit, timeout or server error as well as a 404) is `None`
    /// instead of failing the call, with one summary warning on stderr. For callers
    /// where missing films only make an estimate less informed. After
    /// [`MAX_FILMOGRAPHY_FAILURES`] failures it stops asking TMDB and only uses the
    /// cache, so a broken connection can't send hundreds of doomed requests.
    pub async fn filmographies_or_skip(&self, people: &[u64]) -> Vec<Option<Vec<CreditedFilm>>> {
        let failures = AtomicUsize::new(0);
        let first_error = Mutex::new(None);
        let failures = &failures;
        let first_error = &first_error;
        // Owned ids keep the futures `Send` (see `recommend::resolve`).
        let found: Vec<_> = stream::iter(people.to_vec())
            .map(|id| async move {
                if failures.load(Ordering::Relaxed) >= MAX_FILMOGRAPHY_FAILURES {
                    return self.cached_filmography(id);
                }
                match self.one_filmography(id).await {
                    Ok(films) => Some(films),
                    Err(err) => {
                        if !tmdb::is_not_found(&err) {
                            failures.fetch_add(1, Ordering::Relaxed);
                            if let Ok(mut first) = first_error.lock() {
                                first.get_or_insert(format!("{err:#}"));
                            }
                        }
                        None
                    }
                }
            })
            .buffered(CONCURRENT_REQUESTS)
            .collect()
            .await;
        let failed = failures.load(Ordering::Relaxed);
        if failed > 0 {
            let first = first_error
                .lock()
                .ok()
                .and_then(|mut first| first.take())
                .unwrap_or_default();
            let stopped = if failed >= MAX_FILMOGRAPHY_FAILURES {
                ", so stopped asking TMDB and used only cached ones"
            } else {
                ""
            };
            eprintln!(
                "warning: couldn't fetch films for {failed} of {} people{stopped}; their track \
                 records are left out ({first})",
                people.len()
            );
        }
        found
    }

    fn cached_filmography(&self, person: u64) -> Option<Vec<CreditedFilm>> {
        let key = filmography_cache_key(person);
        self.cache().and_then(|c| c.get(&key, FILMOGRAPHY_MAX_AGE))
    }

    async fn one_filmography(&self, person: u64) -> Result<Vec<CreditedFilm>> {
        let key = filmography_cache_key(person);
        if let Some(films) = self.cached_filmography(person) {
            return Ok(films);
        }
        let films = self
            .client
            .person_movie_credits(person)
            .await
            .with_context(|| format!("fetching films for TMDB person {person}"))?
            .films();
        if let Some(cache) = self.cache() {
            cache.put_or_warn(&key, &films);
        }
        Ok(films)
    }
}

/// The cache key for one person's trimmed filmography.
pub fn filmography_cache_key(person: u64) -> String {
    format!("filmography-v1/{person}")
}

/// The cache key for one movie's details.
pub fn details_cache_key(id: u64) -> String {
    // Bumped whenever MovieDetails keeps a new field, so older entries without it
    // are fetched again. v2: overview and poster. v3: TMDB audience rating.
    format!("details-v3/{id}")
}
