//! Runtime settings, read from environment variables or a `.env` file.

use anyhow::{Context, Result};

pub struct Config {
    pub tmdb_api_key: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();
        let tmdb_api_key =
            std::env::var("TMDB_API_KEY").context("TMDB_API_KEY is not set (see .env.example)")?;
        Ok(Self { tmdb_api_key })
    }
}
