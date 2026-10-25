use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use movie_match::config::Config;
use movie_match::profile;
use movie_match::tmdb::TmdbClient;

/// Rate upcoming movies by how likely you are to enjoy them.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// TOML file listing movies you like.
    #[arg(short, long, default_value = "data/liked_movies.toml")]
    liked: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::from_env()?;

    let liked = profile::load_liked_movies(&cli.liked)?;
    println!("Loaded {} liked movies", liked.len());

    let client = TmdbClient::new(config.tmdb_api_key);
    let upcoming = client.upcoming().await?;
    println!("Found {} upcoming releases", upcoming.len());

    // TODO: look up liked movies on TMDB, build a TasteProfile,
    // then score and rank the upcoming releases.

    Ok(())
}
