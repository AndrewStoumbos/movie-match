//! Imports ratings exported from other sites.

use anyhow::{bail, Context, Result};

use crate::profile::RatedMovie;

/// Parses a Letterboxd `ratings.csv` export (from Settings → Import & Export).
///
/// Letterboxd ratings are 0.5 to 5 stars, so they're doubled onto the 1 to 10
/// scale. Rows without a rating, as in `watched.csv`, are skipped.
pub fn parse_letterboxd_csv(text: &str) -> Result<Vec<RatedMovie>> {
    let text = text.trim_start_matches('\u{feff}');
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    let headers = reader
        .headers()
        .context("could not read the CSV header")?
        .clone();
    let column = |name: &str| {
        headers
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
    };
    let (Some(name_col), Some(rating_col)) = (column("Name"), column("Rating")) else {
        bail!(
            "this doesn't look like a Letterboxd ratings export: it needs \"Name\" and \
             \"Rating\" columns (use ratings.csv from the export)"
        );
    };
    let year_col = column("Year");

    let mut movies = Vec::new();
    for (i, record) in reader.records().enumerate() {
        // Row 1 is the header.
        let row = i + 2;
        let record = record.with_context(|| format!("could not read row {row}"))?;
        let title = record.get(name_col).unwrap_or_default().trim();
        let rating = record.get(rating_col).unwrap_or_default().trim();
        if title.is_empty() || rating.is_empty() {
            continue;
        }
        let stars: f32 = rating
            .parse()
            .with_context(|| format!("row {row} ({title}): rating \"{rating}\" isn't a number"))?;
        if !(0.5..=5.0).contains(&stars) {
            bail!("row {row} ({title}): rating {stars} is outside 0.5 to 5 stars");
        }
        let year = year_col
            .and_then(|col| record.get(col))
            .and_then(|year| year.trim().parse().ok());
        movies.push(RatedMovie {
            tmdb_id: None,
            title: Some(title.to_string()),
            year,
            rating: Some(stars * 2.0),
        });
    }
    Ok(movies)
}
