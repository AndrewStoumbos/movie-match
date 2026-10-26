use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use movie_match::cache::DiskCache;
use movie_match::catalog::CatalogOptions;
use movie_match::config::Config;
use movie_match::fetch::Fetcher;
use movie_match::profile::{self, DISLIKED_UP_TO, LIKED_FROM};
use movie_match::recommend::{self, Options};
use movie_match::tmdb::TmdbClient;

/// Rate upcoming movies by how likely you are to enjoy them.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// TOML file listing movies you've rated (see data/liked_movies.example.toml).
    #[arg(short, long, default_value = "data/liked_movies.toml")]
    liked: PathBuf,

    /// Pages of upcoming releases to rate (20 movies per page).
    #[arg(long, default_value_t = 2)]
    pages: u32,

    /// Pages of popular movies used to judge how rare each feature is.
    #[arg(long, default_value_t = 5)]
    reference_pages: u32,

    /// How many results to show.
    #[arg(short = 'n', long, default_value_t = 20)]
    top: usize,

    /// Where downloaded TMDB data is cached between runs.
    #[arg(long, default_value = "data/cache")]
    cache_dir: PathBuf,

    /// Ignore cached data and download everything again.
    #[arg(long)]
    refresh: bool,

    /// Don't read or write the cache at all.
    #[arg(long, conflicts_with = "refresh")]
    no_cache: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::from_env()?;
    let movies = profile::load_rated_movies(&cli.liked)?;

    let cache = (!cli.no_cache).then(|| {
        let cache = DiskCache::new(&cli.cache_dir);
        if cli.refresh {
            cache.refreshing()
        } else {
            cache
        }
    });
    let fetcher = Fetcher::new(TmdbClient::new(config.tmdb_api_key), cache);
    let options = Options {
        catalog: CatalogOptions {
            upcoming_pages: cli.pages,
            reference_pages: cli.reference_pages,
        },
        ..Options::default()
    };
    let ranking = recommend::rank_upcoming(&fetcher, &movies, &options).await?;
    let resolved = &ranking.resolved;

    let liked = resolved.rated.iter().filter(|r| r.rating >= LIKED_FROM);
    let disliked = resolved.rated.iter().filter(|r| r.rating <= DISLIKED_UP_TO);
    println!(
        "Using {} rated movies: {} liked, {} disliked.",
        resolved.rated.len(),
        liked.count(),
        disliked.count()
    );
    if !resolved.searched.is_empty() {
        println!("Found by title search (add a tmdb_id if any are wrong):");
        for (wanted, found) in &resolved.searched {
            println!("  \"{wanted}\" -> {found}");
        }
    }
    for title in &resolved.unmatched {
        eprintln!("warning: no TMDB match for \"{title}\"");
    }

    println!("\nUpcoming releases, best match first:");
    for ranked in ranking.ranked.iter().take(cli.top) {
        println!(
            "  {:>3}  {} [{}]",
            ranked.score, ranked.movie.title, ranked.movie.release_date
        );
        if let Some((title, rating)) = &ranked.closest_liked {
            let because = if ranked.reasons.is_empty() {
                String::new()
            } else {
                format!(": {}", ranked.reasons.join(", "))
            };
            println!("       like {title} ({}/10){because}", rating_text(*rating));
        }
        if let Some((title, rating)) = &ranked.closest_disliked {
            println!(
                "       but resembles {title}, which you rated {}/10",
                rating_text(*rating)
            );
        }
    }

    Ok(())
}

/// `9` for whole ratings, `7.5` otherwise.
fn rating_text(rating: f32) -> String {
    if rating.fract() == 0.0 {
        format!("{rating:.0}")
    } else {
        format!("{rating:.1}")
    }
}
