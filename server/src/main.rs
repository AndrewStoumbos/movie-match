//! Runs the Movie Match server. The TMDB key comes from `TMDB_API_KEY` (or `.env`).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use movie_match::cache::DiskCache;
use movie_match::config::Config;
use movie_match_server::{router, Server, TMDB_URL};

/// Answer Movie Match's TMDB requests with this server's API key.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Address to listen on. The default only accepts connections from this computer.
    #[arg(long, default_value = "127.0.0.1:8765")]
    listen: SocketAddr,

    /// Where TMDB answers are cached between runs.
    #[arg(long, default_value = "data/server-cache")]
    cache_dir: PathBuf,

    /// Don't cache TMDB answers.
    #[arg(long)]
    no_cache: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::from_env()?;
    let cache = (!cli.no_cache).then(|| DiskCache::new(&cli.cache_dir));
    let server = Server::new(config.tmdb_api_key, TMDB_URL.to_string(), cache)?;
    let listener = tokio::net::TcpListener::bind(cli.listen)
        .await
        .with_context(|| format!("could not listen on {}", cli.listen))?;
    if !cli.listen.ip().is_loopback() {
        eprintln!(
            "warning: listening beyond this computer. Anyone who can reach {} uses this \
             server's TMDB key, and there's no rate limiting yet.",
            cli.listen
        );
    }
    eprintln!(
        "Movie Match server on http://{} (set it as the server in the app)",
        cli.listen
    );
    axum::serve(listener, router(Arc::new(server)))
        .await
        .context("server stopped")
}
