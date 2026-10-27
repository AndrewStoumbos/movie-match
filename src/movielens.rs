//! MovieLens ratings (`ratings.csv` and `links.csv` from a GroupLens download), for
//! evaluating the scoring across many users. Pure: callers read the files. The
//! data's licence forbids redistributing it, so it stays in a git-ignored folder.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{bail, Context, Result};

/// Each user's ratings, most recent first, as (TMDB id, rating 1 to 10).
pub type UserRatings = BTreeMap<u32, Vec<(u64, f32)>>;

/// MovieLens movie id → TMDB id, from `links.csv` (`movieId,imdbId,tmdbId`).
/// Movies without a TMDB id are left out.
pub fn parse_links(text: &str) -> Result<HashMap<u64, u64>> {
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    let mut links = HashMap::new();
    for (i, record) in reader.records().enumerate() {
        // Row 1 is the header.
        let row = i + 2;
        let record = record.with_context(|| format!("could not read links.csv row {row}"))?;
        let movie: u64 = field(&record, 0)
            .parse()
            .with_context(|| format!("links.csv row {row}: bad movieId"))?;
        if let Some(tmdb) = field(&record, 2).parse::<u64>().ok().filter(|id| *id > 0) {
            links.insert(movie, tmdb);
        }
    }
    Ok(links)
}

/// Each user's `per_user` most recent ratings, from `ratings.csv`
/// (`userId,movieId,rating,timestamp`). Ratings are 0.5 to 5 stars, doubled onto
/// the 1 to 10 scale. Movies missing from `links` are skipped, and a TMDB movie
/// linked from two MovieLens ids keeps its more recent rating. Ties in time are
/// broken by TMDB id, so the result doesn't depend on file order.
pub fn recent_ratings(
    text: &str,
    links: &HashMap<u64, u64>,
    per_user: usize,
) -> Result<UserRatings> {
    let mut by_user: BTreeMap<u32, Vec<(u64, u64, f32)>> = BTreeMap::new();
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    for (i, record) in reader.records().enumerate() {
        let row = i + 2;
        let record = record.with_context(|| format!("could not read ratings.csv row {row}"))?;
        let user: u32 = field(&record, 0)
            .parse()
            .with_context(|| format!("ratings.csv row {row}: bad userId"))?;
        let movie: u64 = field(&record, 1)
            .parse()
            .with_context(|| format!("ratings.csv row {row}: bad movieId"))?;
        let stars: f32 = field(&record, 2)
            .parse()
            .with_context(|| format!("ratings.csv row {row}: bad rating"))?;
        if !(0.5..=5.0).contains(&stars) {
            bail!("ratings.csv row {row}: rating {stars} is outside 0.5 to 5 stars");
        }
        let timestamp: u64 = field(&record, 3)
            .parse()
            .with_context(|| format!("ratings.csv row {row}: bad timestamp"))?;
        if let Some(&tmdb) = links.get(&movie) {
            by_user
                .entry(user)
                .or_default()
                .push((timestamp, tmdb, stars * 2.0));
        }
    }
    Ok(by_user
        .into_iter()
        .map(|(user, mut ratings)| {
            ratings.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let mut seen = HashSet::new();
            let recent = ratings
                .into_iter()
                .filter(|(_, tmdb, _)| seen.insert(*tmdb))
                .take(per_user)
                .map(|(_, tmdb, rating)| (tmdb, rating))
                .collect();
            (user, recent)
        })
        .collect())
}

fn field(record: &csv::StringRecord, index: usize) -> &str {
    record.get(index).unwrap_or_default().trim()
}
