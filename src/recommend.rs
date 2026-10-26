//! The per-user steps: resolve a user's ratings to TMDB movies, then rank the
//! shared catalog against them.

use std::collections::HashSet;

use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};

use crate::catalog::{Catalog, CatalogOptions};
use crate::features::FieldWeights;
use crate::fetch::{Fetcher, CONCURRENT_REQUESTS};
use crate::profile::{RatedDetails, RatedMovie, TasteProfile};
use crate::scoring::{self, Explanation, Ranked};
use crate::tmdb::MovieDetails;

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub catalog: CatalogOptions,
    pub weights: FieldWeights,
}

/// A user's ratings matched to TMDB movies.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub rated: Vec<RatedDetails>,
    /// Entries matched by title search, as (what was searched, what TMDB found).
    /// Worth double-checking: the top search result can be the wrong movie.
    pub searched: Vec<(String, String)>,
    /// Entries TMDB search found nothing for.
    pub unmatched: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Ranking {
    pub resolved: Resolved,
    /// Upcoming releases the user hasn't rated, best match first.
    pub ranked: Vec<Ranked>,
}

/// Ranks the catalog's upcoming releases for one user.
pub async fn rank_upcoming(
    fetcher: &Fetcher,
    movies: &[RatedMovie],
    options: &Options,
) -> Result<Ranking> {
    let catalog = Catalog::load(fetcher, options.catalog).await?;
    let resolved = resolve(fetcher, movies).await?;
    let profile = TasteProfile::new(
        &resolved.rated,
        catalog.idf(&options.weights),
        options.weights,
    );
    let ranked = scoring::rank(&profile, &catalog.upcoming);
    Ok(Ranking { resolved, ranked })
}

/// Explains one movie's score for this user. `title`, trimmed of surrounding
/// whitespace, is matched against the upcoming releases first (ignoring case), then
/// looked up with a TMDB search, so any movie works. `None` when nothing matches.
pub async fn explain_movie(
    fetcher: &Fetcher,
    movies: &[RatedMovie],
    options: &Options,
    title: &str,
) -> Result<Option<(MovieDetails, Explanation)>> {
    let title = title.trim();
    let catalog = Catalog::load(fetcher, options.catalog).await?;
    let movie = match catalog
        .upcoming
        .iter()
        .find(|m| m.title.eq_ignore_ascii_case(title))
    {
        Some(movie) => movie.clone(),
        None => {
            let found = fetcher
                .client()
                .search_movie(title, None)
                .await
                .with_context(|| format!("searching TMDB for \"{title}\""))?;
            let Some(found) = found else {
                return Ok(None);
            };
            let mut details = fetcher.details(&[found.id]).await?;
            match details.pop() {
                Some(movie) => movie,
                None => return Ok(None),
            }
        }
    };
    let resolved = resolve(fetcher, movies).await?;
    let profile = TasteProfile::new(
        &resolved.rated,
        catalog.idf(&options.weights),
        options.weights,
    );
    let explanation = scoring::explain(&profile, &movie);
    Ok(Some((movie, explanation)))
}

/// Matches each rated movie to TMDB (by id, or else by title search) and fetches
/// its details. A movie listed twice keeps its first rating.
pub async fn resolve(fetcher: &Fetcher, movies: &[RatedMovie]) -> Result<Resolved> {
    let lookups: Vec<_> = stream::iter(movies)
        .map(|movie| async move { (movie, lookup_id(fetcher, movie).await) })
        .buffered(CONCURRENT_REQUESTS)
        .collect()
        .await;

    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    let mut ratings = Vec::new();
    let mut resolved = Resolved::default();
    for (wanted, found) in lookups {
        let found = found.with_context(|| format!("looking up \"{}\" on TMDB", wanted.label()))?;
        let Some((id, search_match)) = found else {
            resolved.unmatched.push(wanted.label());
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        ids.push(id);
        ratings.push(wanted.rating());
        if let Some(search_match) = search_match {
            resolved.searched.push((wanted.label(), search_match));
        }
    }

    let details = fetcher.details(&ids).await?;
    resolved.rated = details
        .into_iter()
        .zip(ratings)
        .map(|(movie, rating)| RatedDetails { movie, rating })
        .collect();
    Ok(resolved)
}

/// The TMDB id for `movie`, plus a description of the match if a title search
/// was needed to find it.
async fn lookup_id(fetcher: &Fetcher, movie: &RatedMovie) -> Result<Option<(u64, Option<String>)>> {
    if let Some(id) = movie.tmdb_id {
        return Ok(Some((id, None)));
    }
    let Some(title) = movie.title.as_deref() else {
        return Ok(None);
    };
    let found = fetcher.client().search_movie(title, movie.year).await?;
    Ok(found.map(|m| {
        let year = m.release_date.get(..4).unwrap_or("unknown year");
        (m.id, Some(format!("{} ({year})", m.title)))
    }))
}
