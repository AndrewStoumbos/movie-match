use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use movie_match::config::Config;
use movie_match::profile;
use movie_match::recommend::{self, Options};
use movie_match::tmdb::TmdbClient;

/// Rate upcoming movies by how likely you are to enjoy them.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// TOML file listing movies you like.
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::from_env()?;
    let liked = profile::load_liked_movies(&cli.liked)?;

    let client = TmdbClient::new(config.tmdb_api_key);
    let options = Options {
        upcoming_pages: cli.pages,
        reference_pages: cli.reference_pages,
        ..Options::default()
    };
    let ranking = recommend::rank_upcoming(&client, &liked, &options).await?;

    println!("Movies you like, as found on TMDB:");
    for movie in &ranking.liked {
        println!("  {} ({})", movie.title, year(&movie.release_date));
    }
    for title in &ranking.unmatched {
        eprintln!("  warning: no TMDB match for \"{title}\"");
    }

    println!("\nUpcoming releases, best match first:");
    for ranked in ranking.ranked.iter().take(cli.top) {
        let closest = ranked
            .closest_liked
            .as_deref()
            .map(|title| format!("  (closest to {title})"))
            .unwrap_or_default();
        println!(
            "  {:>3}  {} [{}]{closest}",
            ranked.score, ranked.movie.title, ranked.movie.release_date
        );
    }

    Ok(())
}

fn year(release_date: &str) -> &str {
    release_date.get(..4).unwrap_or("unknown year")
}
