//! Where the app keeps its files: settings, imported ratings and the TMDB download
//! cache, all in the user's app-data folder (`%APPDATA%\movie-match\data` on Windows).

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use movie_match::profile::RatedMovie;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const SETTINGS_FILE: &str = "settings.json";
const RATINGS_FILE: &str = "ratings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// TMDB API key. Kept only in the settings file on this computer.
    pub api_key: String,
    /// Pages of upcoming releases to rate (20 movies per page).
    pub upcoming_pages: u32,
    /// Pages of popular movies used to judge how rare each feature is.
    pub reference_pages: u32,
    /// Reuse downloaded TMDB data between runs.
    pub use_cache: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            upcoming_pages: 2,
            reference_pages: 5,
            use_cache: true,
        }
    }
}

/// The user's imported ratings. TMDB ids are filled in after the first match, so
/// later runs skip the title searches.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedRatings {
    /// Name of the file they were imported from.
    pub source: String,
    pub movies: Vec<RatedMovie>,
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open() -> Result<Self> {
        let dirs = ProjectDirs::from("", "", "movie-match")
            .context("could not find a folder to keep app data in")?;
        let dir = dirs.data_dir().to_path_buf();
        fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.dir.join("cache")
    }

    pub fn load_settings(&self) -> Result<Settings> {
        Ok(self.load(SETTINGS_FILE)?.unwrap_or_default())
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.save(SETTINGS_FILE, settings)
    }

    pub fn load_ratings(&self) -> Result<Option<SavedRatings>> {
        self.load(RATINGS_FILE)
    }

    pub fn save_ratings(&self, ratings: &SavedRatings) -> Result<()> {
        self.save(RATINGS_FILE, ratings)
    }

    /// Deletes every cached TMDB download. The next ranking downloads them again.
    pub fn clear_cache(&self) -> Result<()> {
        let cache = self.cache_dir();
        match fs::remove_dir_all(&cache) {
            Err(err) if err.kind() != ErrorKind::NotFound => {
                Err(err).with_context(|| format!("could not delete {}", cache.display()))
            }
            _ => Ok(()),
        }
    }

    fn load<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let path = self.dir.join(name);
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .with_context(|| format!("could not read {}", path.display())),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("could not read {}", path.display())),
        }
    }

    fn save<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        let path = self.dir.join(name);
        let json = serde_json::to_string_pretty(value).context("could not encode app data")?;
        // Write to a temporary file and rename it, so a crash never leaves half a file.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).with_context(|| format!("could not write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("could not write {}", path.display()))
    }
}
