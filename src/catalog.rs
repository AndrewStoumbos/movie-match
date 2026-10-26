//! The part of a ranking that's the same for every user: the candidate movies and
//! the reference set used to judge how rare each feature is. Fetched once and
//! cached for a day.

use std::collections::HashSet;
use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::features::{self, FieldWeights};
use crate::fetch::Fetcher;
use crate::tfidf::Idf;
use crate::tmdb::{Movie, MovieDetails, Page};

/// How long the cached lists of upcoming and popular movies are reused.
pub const CATALOG_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy)]
pub struct CatalogOptions {
    /// Pages of `/movie/upcoming` to rate (20 movies per page).
    pub upcoming_pages: u32,
    /// Pages of `/movie/popular` in the rarity reference set.
    pub reference_pages: u32,
}

impl Default for CatalogOptions {
    fn default() -> Self {
        Self {
            upcoming_pages: 2,
            reference_pages: 5,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// Movies releasing soon: the candidates for "upcoming for you".
    pub upcoming: Vec<MovieDetails>,
    /// Popular movies, used only to judge how rare each feature is.
    pub reference: Vec<MovieDetails>,
}

/// What's cached for the catalog: just the ids. Details live in the details cache.
#[derive(Debug, Serialize, Deserialize)]
struct CatalogIds {
    upcoming: Vec<u64>,
    reference: Vec<u64>,
}

impl Catalog {
    /// The catalog from cache if it's less than [`CATALOG_MAX_AGE`] old, otherwise
    /// freshly listed from TMDB.
    pub async fn load(fetcher: &Fetcher, options: CatalogOptions) -> Result<Self> {
        let key = format!(
            "catalog-u{}-r{}",
            options.upcoming_pages, options.reference_pages
        );
        let ids = match fetcher.cache().and_then(|c| c.get(&key, CATALOG_MAX_AGE)) {
            Some(ids) => ids,
            None => {
                let ids = list_catalog(fetcher, options).await?;
                if let Some(cache) = fetcher.cache() {
                    cache.put_or_warn(&key, &ids);
                }
                ids
            }
        };
        // The ids may be a day old, so a movie TMDB has removed since is skipped
        // rather than failing every run until the cached list expires. Any other
        // failure fails the load, so a network blip can't skew the catalog.
        Ok(Self {
            upcoming: fetcher.available_details(&ids.upcoming).await?,
            reference: fetcher.available_details(&ids.reference).await?,
        })
    }

    /// Rarity of each feature across every catalog movie.
    pub fn idf(&self, weights: &FieldWeights) -> Idf {
        let documents: Vec<_> = self
            .upcoming
            .iter()
            .chain(&self.reference)
            .map(|movie| features::extract(movie, weights))
            .collect();
        Idf::from_documents(&documents)
    }
}

async fn list_catalog(fetcher: &Fetcher, options: CatalogOptions) -> Result<CatalogIds> {
    let client = fetcher.client();
    // Every id appears once, even if it's both upcoming and popular.
    let mut seen = HashSet::new();
    let upcoming = list_ids(options.upcoming_pages, &mut seen, |p| client.upcoming(p)).await?;
    let reference = list_ids(options.reference_pages, &mut seen, |p| client.popular(p)).await?;
    Ok(CatalogIds {
        upcoming,
        reference,
    })
}

/// Ids from the first `pages` pages of a TMDB list, skipping any already in `seen`.
async fn list_ids<F, Fut>(pages: u32, seen: &mut HashSet<u64>, fetch_page: F) -> Result<Vec<u64>>
where
    F: Fn(u32) -> Fut,
    Fut: Future<Output = Result<Page<Movie>>>,
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
