//! The part of a ranking that's the same for every user: the candidate movies
//! (releasing today or later, and released recently) and the reference set used to
//! judge how rare each feature is. Fetched once a day and cached.

use std::collections::HashSet;
use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::dates::Date;
use crate::features::{self, FieldWeights};
use crate::fetch::Fetcher;
use crate::tfidf::Idf;
use crate::tmdb::{Discover, Movie, MovieDetails, Page, Region};

/// How long the cached lists of upcoming, recent and popular movies are reused.
pub const CATALOG_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Recent releases are fetched in slices of this many months, the most popular of
/// each, so the list covers the whole window rather than just its newest weeks.
pub const RECENT_SLICE_MONTHS: u32 = 3;

#[derive(Debug, Clone, Copy)]
pub struct CatalogOptions {
    /// Pages of movies releasing today or later to rate, most popular first (20 per page).
    pub upcoming_pages: u32,
    /// Only upcoming movies with a cinema release listed for this country, which
    /// leaves out films going straight to streaming or only showing elsewhere.
    /// `None` takes every upcoming movie on TMDB.
    pub upcoming_region: Option<Region>,
    /// Pages of recent releases to rate per [`RECENT_SLICE_MONTHS`] slice (20 per page).
    pub recent_pages: u32,
    /// How far back "recent" reaches, in months.
    pub recent_months: u32,
    /// Recent releases need at least this many TMDB audience votes, so the list
    /// sticks to films people have actually seen.
    pub recent_min_votes: u32,
    /// Pages of `/movie/popular` in the rarity reference set.
    pub reference_pages: u32,
}

impl Default for CatalogOptions {
    fn default() -> Self {
        Self {
            upcoming_pages: 2,
            upcoming_region: None,
            recent_pages: 1,
            recent_months: 12,
            recent_min_votes: 50,
            reference_pages: 5,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// Movies releasing today or later: the candidates for "upcoming for you".
    pub upcoming: Vec<MovieDetails>,
    /// Movies released before today, within the recent window.
    pub recent: Vec<MovieDetails>,
    /// Popular movies, used only to judge how rare each feature is.
    pub reference: Vec<MovieDetails>,
}

/// What's cached for the catalog: just the ids. Details live in the details cache.
#[derive(Debug, Serialize, Deserialize)]
struct CatalogIds {
    /// The day the lists were made for (`YYYY-MM-DD`). Lists from another day are
    /// a cache miss, since where upcoming ends and recent begins moves every day.
    listed_on: String,
    upcoming: Vec<u64>,
    recent: Vec<u64>,
    reference: Vec<u64>,
}

/// The cache key for the catalog with `options`. The day isn't part of it (it's
/// stored with the ids), so each set of options keeps one file that's replaced
/// when the day changes. Use it in tests, never a literal key.
pub fn catalog_cache_key(options: &CatalogOptions) -> String {
    format!(
        "catalog-v4-u{}-{}-c{}-m{}-v{}-r{}",
        options.upcoming_pages,
        options
            .upcoming_region
            .map_or_else(|| "all".to_string(), |r| r.to_string()),
        options.recent_pages,
        options.recent_months,
        options.recent_min_votes,
        options.reference_pages
    )
}

/// The release dates each list covers, as `YYYY-MM-DD` (which compares correctly
/// as text). Upcoming is `upcoming_from` onward; recent is `recent_from` to
/// `recent_to` inclusive, ending the day before upcoming starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseWindows {
    pub upcoming_from: String,
    pub recent_from: String,
    pub recent_to: String,
    /// The recent window in [`RECENT_SLICE_MONTHS`] slices, newest first, as
    /// inclusive `(from, to)` ranges that together cover `recent_from..=recent_to`.
    pub recent_slices: Vec<(String, String)>,
}

impl ReleaseWindows {
    /// The windows as of `today` (days since 1970-01-01), with recent reaching back
    /// `recent_months` months.
    pub fn new(today: i64, recent_months: u32) -> Self {
        let date = Date::from_unix_days(today);
        let mut recent_slices = Vec::new();
        // Each slice ends the day before the newer one starts; the newest ends yesterday.
        let mut next_start = date;
        let mut back = 0;
        while back < recent_months {
            back = (back + RECENT_SLICE_MONTHS).min(recent_months);
            let start = date.months_earlier(back);
            let end = Date::from_unix_days(next_start.to_unix_days() - 1);
            recent_slices.push((start.to_string(), end.to_string()));
            next_start = start;
        }
        Self {
            upcoming_from: date.to_string(),
            recent_from: date.months_earlier(recent_months).to_string(),
            recent_to: Date::from_unix_days(today - 1).to_string(),
            recent_slices,
        }
    }
}

/// `movies` released on or after `from` and, when given, on or before `to`
/// (`YYYY-MM-DD`). Movies without a release date are left out.
pub fn released_between(
    movies: Vec<MovieDetails>,
    from: &str,
    to: Option<&str>,
) -> Vec<MovieDetails> {
    movies
        .into_iter()
        .filter(|m| {
            let date = m.release_date.as_str();
            !date.is_empty() && date >= from && to.is_none_or(|to| date <= to)
        })
        .collect()
}

/// Today as days since 1970-01-01, by the computer's local calendar date (not UTC),
/// so a film counts as upcoming on its release day wherever the user is.
pub fn today() -> i64 {
    use chrono::Datelike;
    // num_days_from_ce counts 0001-01-01 as day 1; 1970-01-01 is day 719,163.
    i64::from(chrono::Local::now().date_naive().num_days_from_ce()) - 719_163
}

impl Catalog {
    /// The catalog from cache if it's less than [`CATALOG_MAX_AGE`] old, otherwise
    /// freshly listed from TMDB.
    pub async fn load(fetcher: &Fetcher, options: CatalogOptions) -> Result<Self> {
        let windows = ReleaseWindows::new(today(), options.recent_months);
        let key = catalog_cache_key(&options);
        let cached = fetcher
            .cache()
            .and_then(|c| c.get::<CatalogIds>(&key, CATALOG_MAX_AGE))
            .filter(|ids| ids.listed_on == windows.upcoming_from);
        let ids = match cached {
            Some(ids) => ids,
            None => {
                let ids = list_catalog(fetcher, options, &windows).await?;
                if let Some(cache) = fetcher.cache() {
                    cache.put_or_warn(&key, &ids);
                }
                ids
            }
        };
        // The ids may be a day old, so a movie TMDB has removed since is skipped
        // rather than failing every run until the cached list expires. Any other
        // failure fails the load, so a network blip can't skew the catalog. The
        // release dates are checked again here, since a film's date can change.
        // Vote counts aren't: discover already applied the minimum, counts only
        // grow, and cached details can be days older than the list.
        Ok(Self {
            upcoming: released_between(
                fetcher.available_details(&ids.upcoming).await?,
                &windows.upcoming_from,
                None,
            ),
            recent: released_between(
                fetcher.available_details(&ids.recent).await?,
                &windows.recent_from,
                Some(&windows.recent_to),
            ),
            reference: fetcher.available_details(&ids.reference).await?,
        })
    }

    /// Rarity of each feature across every catalog movie.
    pub fn idf(&self, weights: &FieldWeights) -> Idf {
        let documents: Vec<_> = self
            .upcoming
            .iter()
            .chain(&self.recent)
            .chain(&self.reference)
            .map(|movie| features::extract(movie, weights))
            .collect();
        Idf::from_documents(&documents)
    }
}

async fn list_catalog(
    fetcher: &Fetcher,
    options: CatalogOptions,
    windows: &ReleaseWindows,
) -> Result<CatalogIds> {
    let client = fetcher.client();
    // Every id appears once, even if it's both recent (or upcoming) and popular.
    let mut seen = HashSet::new();
    let upcoming = list_ids(options.upcoming_pages, &mut seen, |p| {
        let query = Discover {
            from: Some(&windows.upcoming_from),
            theatrical_in: options.upcoming_region,
            ..Discover::default()
        };
        client.discover(query, p)
    })
    .await?;
    // Popularity favours the newest releases, so each slice gets its own pages.
    let mut recent = Vec::new();
    for (from, to) in &windows.recent_slices {
        recent.extend(
            list_ids(options.recent_pages, &mut seen, |p| {
                let query = Discover {
                    from: Some(from),
                    to: Some(to),
                    min_votes: options.recent_min_votes,
                    ..Discover::default()
                };
                client.discover(query, p)
            })
            .await?,
        );
    }
    let reference = list_ids(options.reference_pages, &mut seen, |p| client.popular(p)).await?;
    Ok(CatalogIds {
        listed_on: windows.upcoming_from.clone(),
        upcoming,
        recent,
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
