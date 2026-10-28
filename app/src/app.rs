//! The app's state and screens.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, RichText};
use movie_match::cache::DiskCache;
use movie_match::catalog::CatalogOptions;
use movie_match::fetch::Fetcher;
use movie_match::import;
use movie_match::profile::{self, RatedMovie, DISLIKED_UP_TO, LIKED_FROM};
use movie_match::quality::Role;
use movie_match::recommend::{self, Options, Ranking, Stage};
use movie_match::scoring::{AudienceView, Basis, Breakdown, Explanation, Ranked, TrackRecordNudge};
use movie_match::tmdb::{MovieDetails, Region, TmdbClient};
use tokio::runtime::Runtime;

use crate::jobs::Job;
use crate::store::{SavedRatings, Settings, Store};

const POSTER_SIZE: egui::Vec2 = egui::vec2(72.0, 108.0);
const API_KEY_URL: &str = "https://www.themoviedb.org/settings/api";
const LETTERBOXD_EXPORT_URL: &str = "https://letterboxd.com/settings/data/";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Upcoming,
    Recent,
    Explain,
    Ratings,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShowCount {
    Top20,
    Top50,
    All,
}

impl ShowCount {
    const ALL: [ShowCount; 3] = [ShowCount::Top20, ShowCount::Top50, ShowCount::All];

    fn limit(self) -> usize {
        match self {
            ShowCount::Top20 => 20,
            ShowCount::Top50 => 50,
            ShowCount::All => usize::MAX,
        }
    }

    fn label(self) -> &'static str {
        match self {
            ShowCount::Top20 => "Top 20",
            ShowCount::Top50 => "Top 50",
            ShowCount::All => "All",
        }
    }
}

enum Notice {
    Info(String),
    Error(String),
}

/// Something a screen asked for, carried out after drawing so screens can stay
/// simple borrows of the app state.
enum Action {
    Rank {
        refresh: bool,
    },
    Explain(String),
    Import,
    /// Check a pasted TMDB id against TMDB, then save it for that rating row.
    CheckTmdbId(usize, u64),
    /// Forget a rating row's TMDB match, so it's looked up by title again.
    ClearTmdbId(usize),
    OpenTab(Tab),
}

type Explained = Option<(MovieDetails, Explanation)>;

pub struct MovieMatchApp {
    runtime: Runtime,
    store: Store,
    settings: Settings,
    ratings: Option<SavedRatings>,
    tab: Tab,
    notice: Option<Notice>,

    ranking: Option<Ranking>,
    ranking_job: Option<Job<Ranking>>,
    /// The step the ranking job is on, set from its background thread.
    ranking_stage: Arc<Mutex<Option<Stage>>>,
    filter: String,
    show: ShowCount,

    explain_query: String,
    explained: Explained,
    explain_job: Option<Job<Explained>>,

    /// What's typed in the API key box. The saved key is never shown.
    key_input: String,
    /// Rating row whose TMDB match is being fixed, with the id typed so far.
    editing: Option<(usize, String)>,
    /// A pasted id being checked against TMDB before it's saved: row, id, and the
    /// lookup (`None` if TMDB has no movie with that id).
    checking: Option<(usize, u64, Job<Option<MovieDetails>>)>,
}

impl MovieMatchApp {
    /// Loads saved settings and ratings. `import` is a ratings file to import right
    /// away, e.g. one opened with the app.
    pub fn new(cc: &eframe::CreationContext<'_>, import: Option<PathBuf>) -> Result<Self> {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("could not start background tasks")?;
        let store = Store::open()?;

        let mut notice = None;
        let settings = store.load_settings().unwrap_or_else(|err| {
            notice = Some(Notice::Error(format!("{err:#}")));
            Settings::default()
        });
        let ratings = store.load_ratings().unwrap_or_else(|err| {
            notice = Some(Notice::Error(format!("{err:#}")));
            None
        });

        let mut app = Self {
            runtime,
            store,
            settings,
            ratings,
            tab: Tab::Upcoming,
            notice,
            ranking: None,
            ranking_job: None,
            ranking_stage: Arc::default(),
            filter: String::new(),
            show: ShowCount::Top20,
            explain_query: String::new(),
            explained: None,
            explain_job: None,
            key_input: String::new(),
            editing: None,
            checking: None,
        };
        if let Some(path) = import {
            // Imports, then ranks (or asks for an API key first).
            app.import_file(&cc.egui_ctx, &path);
        } else if !app.has_tmdb() {
            app.tab = Tab::Settings;
        } else if app.ratings.is_some() {
            app.start_ranking(&cc.egui_ctx, false);
        }
        Ok(app)
    }

    /// The saved key, or else `TMDB_API_KEY` from the environment or a `.env` file.
    fn api_key(&self) -> Option<String> {
        let saved = self.settings.api_key.trim();
        if !saved.is_empty() {
            return Some(saved.to_string());
        }
        std::env::var("TMDB_API_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty())
    }

    /// The Movie Match server to use instead of TMDB: the saved one if it looks like a
    /// web address, or else `MOVIE_MATCH_SERVER` from the environment (which
    /// `cargo dev` sets).
    fn server_url(&self) -> Option<String> {
        let web = |url: &str| url.starts_with("http://") || url.starts_with("https://");
        let saved = self.settings.server_url.trim();
        if !saved.is_empty() {
            return web(saved).then(|| saved.to_string());
        }
        std::env::var("MOVIE_MATCH_SERVER")
            .ok()
            .map(|url| url.trim().to_string())
            .filter(|url| web(url))
    }

    /// Whether the app can look movies up: through a server, or with a key.
    fn has_tmdb(&self) -> bool {
        self.server_url().is_some() || self.api_key().is_some()
    }

    fn options(&self) -> Options {
        Options {
            catalog: CatalogOptions {
                upcoming_pages: self.settings.upcoming_pages,
                upcoming_region: Region::parse(&self.settings.upcoming_region),
                recent_pages: self.settings.recent_pages,
                recent_months: self.settings.recent_months,
                reference_pages: self.settings.reference_pages,
                ..CatalogOptions::default()
            },
            ..Options::default()
        }
    }

    /// A TMDB fetcher for one task, or `None` (with a notice) when there's neither a
    /// server nor a key. A server, when set, wins over a key.
    fn fetcher(&mut self, refresh: bool) -> Option<Arc<Fetcher>> {
        let client = match (self.server_url(), self.api_key()) {
            (Some(url), _) => TmdbClient::via_server(&url),
            (None, Some(key)) => TmdbClient::new(key),
            (None, None) => {
                self.notice = Some(Notice::Error(
                    "Add a Movie Match server or a TMDB API key in Settings first.".into(),
                ));
                self.tab = Tab::Settings;
                return None;
            }
        };
        let cache = self.settings.use_cache.then(|| {
            let cache = DiskCache::new(self.store.cache_dir());
            if refresh {
                cache.refreshing()
            } else {
                cache
            }
        });
        Some(Arc::new(Fetcher::new(client, cache)))
    }

    fn start_ranking(&mut self, ctx: &egui::Context, refresh: bool) {
        let Some(movies) = self.ratings.as_ref().map(|r| r.movies.clone()) else {
            return;
        };
        let Some(fetcher) = self.fetcher(refresh) else {
            return;
        };
        let options = self.options();
        let stage = Arc::clone(&self.ranking_stage);
        if let Ok(mut current) = stage.lock() {
            *current = None;
        }
        let repaint = ctx.clone();
        let report = move |now: Stage| {
            if let Ok(mut current) = stage.lock() {
                *current = Some(now);
            }
            repaint.request_repaint();
        };
        self.ranking_job = Some(Job::spawn(&self.runtime, ctx, async move {
            recommend::rank_upcoming_reporting(&fetcher, &movies, &options, report).await
        }));
    }

    fn start_explain(&mut self, ctx: &egui::Context, title: String) {
        let title = title.trim().to_string();
        if title.is_empty() {
            return;
        }
        self.explain_query = title.clone();
        self.tab = Tab::Explain;
        let Some(movies) = self.ratings.as_ref().map(|r| r.movies.clone()) else {
            self.notice = Some(Notice::Error("Import your ratings first.".into()));
            return;
        };
        let Some(fetcher) = self.fetcher(false) else {
            return;
        };
        let options = self.options();
        self.explained = None;
        self.explain_job = Some(Job::spawn(&self.runtime, ctx, async move {
            recommend::explain_movie(&fetcher, &movies, &options, &title).await
        }));
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.ranking_job.as_ref().and_then(Job::poll) {
            self.ranking_job = None;
            match result {
                Ok(ranking) => self.finish_ranking(ranking),
                Err(err) => self.notice = Some(Notice::Error(err)),
            }
        }
        if let Some(result) = self.explain_job.as_ref().and_then(Job::poll) {
            self.explain_job = None;
            match result {
                Ok(Some(found)) => self.explained = Some(found),
                Ok(None) => {
                    self.notice = Some(Notice::Info(format!(
                        "No movie found for \"{}\".",
                        self.explain_query
                    )));
                }
                Err(err) => self.notice = Some(Notice::Error(err)),
            }
        }
        let checked = self
            .checking
            .as_ref()
            .and_then(|(row, id, job)| Some((*row, *id, job.poll()?)));
        if let Some((row, id, result)) = checked {
            self.checking = None;
            match result {
                Ok(Some(movie)) => {
                    self.notice = Some(Notice::Info(format!(
                        "Matched to {} ({}).",
                        movie.title,
                        year(&movie.release_date)
                    )));
                    self.save_tmdb_id(ctx, row, Some(id));
                }
                Ok(None) => {
                    self.notice = Some(Notice::Error(format!(
                        "TMDB has no movie with id {id}. Paste the link from the movie's \
                         themoviedb.org page (it contains /movie/)."
                    )));
                }
                Err(err) => self.notice = Some(Notice::Error(err)),
            }
        }
    }

    /// Keeps the ranking and saves the TMDB ids it found, so the next run skips the
    /// title searches. A saved id TMDB no longer has comes back as `None` and is
    /// dropped, so that rating is looked up by title next time.
    fn finish_ranking(&mut self, ranking: Ranking) {
        if let Some(ratings) = &mut self.ratings {
            if apply_resolved_ids(&mut ratings.movies, &ranking.resolved.ids) {
                if let Err(err) = self.store.save_ratings(ratings) {
                    self.notice = Some(Notice::Error(format!("{err:#}")));
                }
            }
        }
        let unmatched = ranking.resolved.unmatched.len();
        if unmatched > 0 {
            self.notice = Some(Notice::Info(format!(
                "{unmatched} of your ratings couldn't be found on TMDB. You can fix them under My ratings."
            )));
        }
        self.ranking = Some(ranking);
    }

    fn import_file(&mut self, ctx: &egui::Context, path: &Path) {
        let mut movies = match read_ratings_file(path) {
            Ok(movies) if !movies.is_empty() => movies,
            Ok(_) => {
                self.notice = Some(Notice::Error(
                    "No rated movies found in that file. Use ratings.csv from your Letterboxd export."
                        .into(),
                ));
                return;
            }
            Err(err) => {
                self.notice = Some(Notice::Error(format!("{err:#}")));
                return;
            }
        };
        // Reuse TMDB ids already found for the same title and year.
        if let Some(old) = &self.ratings {
            let known: HashMap<(String, Option<u16>), u64> = old
                .movies
                .iter()
                .filter_map(|m| Some(((m.title.clone()?, m.year), m.tmdb_id?)))
                .collect();
            for movie in &mut movies {
                if movie.tmdb_id.is_none() {
                    if let Some(title) = &movie.title {
                        movie.tmdb_id = known.get(&(title.clone(), movie.year)).copied();
                    }
                }
            }
        }
        let source = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let saved = SavedRatings { source, movies };
        if let Err(err) = self.store.save_ratings(&saved) {
            self.notice = Some(Notice::Error(format!("{err:#}")));
            return;
        }
        self.notice = Some(Notice::Info(format!(
            "Imported {} ratings from {}.",
            saved.movies.len(),
            saved.source
        )));
        self.ratings = Some(saved);
        // Jobs started from the old list refer to its rows by position, so their
        // results would land on the wrong movies. Drop them even if no new ranking
        // can start (e.g. no API key).
        self.ranking_job = None;
        self.checking = None;
        self.ranking = None;
        self.explained = None;
        self.editing = None;
        self.tab = Tab::Upcoming;
        self.start_ranking(ctx, false);
    }

    /// Looks a pasted id up on TMDB first, so a wrong or deleted id is never saved.
    fn check_tmdb_id(&mut self, ctx: &egui::Context, row: usize, id: u64) {
        let Some(fetcher) = self.fetcher(false) else {
            return;
        };
        let job = Job::spawn(&self.runtime, ctx, async move {
            let mut found = fetcher.details_if_found(&[id]).await?;
            Ok(found.pop().flatten())
        });
        self.checking = Some((row, id, job));
    }

    /// Saves (or with `None`, clears) one rating's TMDB id, then ranks again.
    fn save_tmdb_id(&mut self, ctx: &egui::Context, row: usize, id: Option<u64>) {
        let Some(ratings) = &mut self.ratings else {
            return;
        };
        let Some(movie) = ratings.movies.get_mut(row) else {
            return;
        };
        movie.tmdb_id = id;
        if let Err(err) = self.store.save_ratings(ratings) {
            self.notice = Some(Notice::Error(format!("{err:#}")));
            return;
        }
        self.editing = None;
        // A running ranking used the old ids; drop it even if no new one can start.
        self.ranking_job = None;
        self.start_ranking(ctx, false);
    }

    fn run(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Rank { refresh } => self.start_ranking(ctx, refresh),
            Action::Explain(title) => self.start_explain(ctx, title),
            Action::Import => {
                let picked = rfd::FileDialog::new()
                    .set_title("Choose your Letterboxd ratings.csv")
                    .add_filter("Letterboxd ratings", &["csv"])
                    .add_filter("movie-match ratings", &["toml"])
                    .pick_file();
                if let Some(path) = picked {
                    self.import_file(ctx, &path);
                }
            }
            Action::CheckTmdbId(row, id) => self.check_tmdb_id(ctx, row, id),
            Action::ClearTmdbId(row) => self.save_tmdb_id(ctx, row, None),
            Action::OpenTab(tab) => self.tab = tab,
        }
    }

    fn save_settings(&mut self) {
        if let Err(err) = self.store.save_settings(&self.settings) {
            self.notice = Some(Notice::Error(format!("{err:#}")));
        }
    }

    fn busy_text(&self) -> Option<String> {
        if self.ranking_job.is_some() {
            Some(self.stage_text())
        } else if self.explain_job.is_some() {
            Some("Working out a score…".into())
        } else if self.checking.is_some() {
            Some("Checking that TMDB id…".into())
        } else {
            None
        }
    }

    /// What the ranking job is doing right now.
    fn stage_text(&self) -> String {
        let stage = self.ranking_stage.lock().ok().and_then(|s| *s);
        match stage {
            None | Some(Stage::Catalog) => "Listing upcoming and recent movies…".into(),
            Some(Stage::Ratings) => "Matching your ratings to TMDB…".into(),
            Some(Stage::TrackRecords { people }) => format!(
                "Looking up {} cast and crew's earlier films…",
                thousands(u32::try_from(people).unwrap_or(u32::MAX))
            ),
            Some(Stage::Scoring) => "Scoring…".into(),
        }
    }

    // ---- Screens ---------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.heading(RichText::new("Movie Match").strong());
            ui.add_space(18.0);
            for (tab, label) in [
                (Tab::Upcoming, "Upcoming"),
                (Tab::Recent, "Recent"),
                (Tab::Explain, "Explain a movie"),
                (Tab::Ratings, "My ratings"),
                (Tab::Settings, "Settings"),
            ] {
                ui.selectable_value(&mut self.tab, tab, RichText::new(label).size(15.0));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(text) = self.busy_text() {
                    ui.label(text);
                    ui.spinner();
                }
            });
        });
        ui.add_space(6.0);
    }

    fn notice_banner(&mut self, ui: &mut egui::Ui) {
        let Some(notice) = &self.notice else {
            return;
        };
        let (text, fill) = match notice {
            Notice::Info(text) => (text.as_str(), Color32::from_rgb(28, 64, 96)),
            Notice::Error(text) => (text.as_str(), Color32::from_rgb(110, 36, 36)),
        };
        let mut dismissed = false;
        egui::Frame::new()
            .fill(fill)
            .corner_radius(6.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(text).color(Color32::WHITE));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Dismiss").clicked() {
                            dismissed = true;
                        }
                    });
                });
            });
        if dismissed {
            self.notice = None;
        }
        ui.add_space(8.0);
    }

    /// What a screen needs before it can show anything: a key, then ratings.
    fn missing_setup(&self) -> Option<Action> {
        if !self.has_tmdb() {
            Some(Action::OpenTab(Tab::Settings))
        } else if self.ratings.is_none() {
            Some(Action::Import)
        } else {
            None
        }
    }

    fn setup_prompt(&self, ui: &mut egui::Ui, needed: &Action) -> Option<Action> {
        let mut action = None;
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            match needed {
                Action::OpenTab(_) => {
                    ui.heading("Add your TMDB API key");
                    ui.add_space(6.0);
                    ui.label("Movie Match looks movies up on TMDB, which needs a free API key.");
                    ui.add_space(12.0);
                    if ui
                        .button(RichText::new("Open Settings").size(16.0))
                        .clicked()
                    {
                        action = Some(Action::OpenTab(Tab::Settings));
                    }
                }
                _ => {
                    ui.heading("Import your Letterboxd ratings");
                    ui.add_space(6.0);
                    ui.label("On Letterboxd, go to Settings → Import & Export → Export your data.");
                    ui.label("Unzip the download and choose ratings.csv.");
                    ui.hyperlink_to("Open Letterboxd's export page", LETTERBOXD_EXPORT_URL);
                    ui.add_space(14.0);
                    if ui
                        .button(RichText::new("Choose ratings.csv…").size(16.0))
                        .clicked()
                    {
                        action = Some(Action::Import);
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new("…or drag the file onto this window.").weak());
                }
            }
        });
        action
    }

    /// The Upcoming (today or later) or, with `recent`, the Recent releases list.
    fn list_tab(&mut self, ui: &mut egui::Ui, recent: bool) -> Option<Action> {
        if let Some(needed) = self.missing_setup() {
            return self.setup_prompt(ui, &needed);
        }
        let mut action = None;
        let idle = self.ranking_job.is_none();
        let months = self.settings.recent_months;
        let kind = if recent { "recent" } else { "upcoming" };

        ui.heading(if recent {
            format!("Recent releases (the last {months} months), ranked for you")
        } else {
            "Upcoming releases, ranked for you".to_string()
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Filter by title")
                    .desired_width(220.0),
            );
            egui::ComboBox::new("show_count", "")
                .selected_text(self.show.label())
                .show_ui(ui, |ui| {
                    for count in ShowCount::ALL {
                        ui.selectable_value(&mut self.show, count, count.label());
                    }
                });
            if ui
                .add_enabled(idle, egui::Button::new("Reload"))
                .on_hover_text("Rank again, reusing downloaded data")
                .clicked()
            {
                action = Some(Action::Rank { refresh: false });
            }
            if ui
                .add_enabled(idle, egui::Button::new("Re-download"))
                .on_hover_text("Fetch fresh movie lists and details from TMDB")
                .clicked()
            {
                action = Some(Action::Rank { refresh: true });
            }
        });
        ui.add_space(6.0);

        let Some(ranking) = &self.ranking else {
            if idle {
                ui.label("Nothing ranked yet.");
                if ui.button("Rank now").clicked() {
                    action = Some(Action::Rank { refresh: false });
                }
            } else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!(
                        "{} The first run can take a minute or two; later runs are quick.",
                        self.stage_text()
                    ));
                });
            }
            return action;
        };

        let list = if recent {
            &ranking.recent
        } else {
            &ranking.ranked
        };
        let rated = &ranking.resolved.rated;
        let liked = rated.iter().filter(|r| r.rating >= LIKED_FROM).count();
        let disliked = rated.iter().filter(|r| r.rating <= DISLIKED_UP_TO).count();
        let which = if recent {
            "released before today"
        } else {
            "out today or later"
        };
        ui.label(
            RichText::new(format!(
                "Based on {} rated movies ({liked} liked, {disliked} disliked) · {} {kind} releases, \
                 {which}. Scores are your predicted rating × 10.",
                rated.len(),
                list.len()
            ))
            .weak(),
        );
        ui.add_space(6.0);

        let filter = self.filter.trim().to_lowercase();
        let limit = self.show.limit();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                let shown = list
                    .iter()
                    .filter(|r| filter.is_empty() || r.movie.title.to_lowercase().contains(&filter))
                    .take(limit);
                let mut any = false;
                for (i, ranked) in shown.enumerate() {
                    any = true;
                    if movie_card(ui, i + 1, ranked) {
                        action = Some(Action::Explain(ranked.movie.title.clone()));
                    }
                }
                if !any {
                    if list.is_empty() {
                        ui.label(format!("No {kind} releases found."));
                    } else {
                        ui.label(format!("No {kind} movies match that filter."));
                    }
                }
            });
        action
    }

    fn explain_tab(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        if let Some(needed) = self.missing_setup() {
            return self.setup_prompt(ui, &needed);
        }
        let mut action = None;
        ui.heading("Explain a movie's score");
        ui.label("Type any title: an upcoming release or any movie on TMDB.");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.explain_query)
                    .hint_text("e.g. Dune: Part Two")
                    .desired_width(320.0),
            );
            let entered = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let clicked = ui
                .add_enabled(self.explain_job.is_none(), egui::Button::new("Explain"))
                .clicked();
            if (entered || clicked) && self.explain_job.is_none() {
                action = Some(Action::Explain(self.explain_query.clone()));
            }
        });
        ui.add_space(8.0);
        if self.explain_job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Working it out…");
            });
        }
        if let Some((movie, explanation)) = &self.explained {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| explanation_view(ui, movie, explanation));
        }
        action
    }

    fn ratings_tab(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let Some(ratings) = &self.ratings else {
            return self.setup_prompt(ui, &Action::Import);
        };
        let mut action = None;
        let movies = &ratings.movies;
        let liked = movies.iter().filter(|m| m.rating() >= LIKED_FROM).count();
        let disliked = movies
            .iter()
            .filter(|m| m.rating() <= DISLIKED_UP_TO)
            .count();
        let neutral = movies.len() - liked - disliked;

        ui.heading("My ratings");
        ui.label(format!(
            "{} ratings from {}: {liked} liked (7+), {neutral} in between, {disliked} disliked (4 or less).",
            movies.len(),
            ratings.source
        ));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Import another file…").clicked() {
                action = Some(Action::Import);
            }
            if ui
                .add_enabled(self.ranking_job.is_none(), egui::Button::new("Match again"))
                .clicked()
            {
                action = Some(Action::Rank { refresh: false });
            }
            ui.hyperlink_to("Letterboxd export page", LETTERBOXD_EXPORT_URL);
        });
        ui.label(
            RichText::new(
                "Wrong or missing match? Click Fix and paste the link from the movie's \
                 themoviedb.org page, or Clear to look it up by title again.",
            )
            .weak(),
        );
        ui.add_space(6.0);

        // TMDB titles for matched ids, once a ranking has run.
        let matched: HashMap<u64, &MovieDetails> = self
            .ranking
            .iter()
            .flat_map(|r| &r.resolved.rated)
            .map(|r| (r.movie.id, &r.movie))
            .collect();
        let ranked_once = self.ranking.is_some();
        let checking = self.checking.is_some();
        let editing = &mut self.editing;

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Grid::new("ratings_grid")
                    .num_columns(4)
                    .striped(true)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Title").strong());
                        ui.label(RichText::new("Your rating").strong());
                        ui.label(RichText::new("Matched on TMDB").strong());
                        ui.label("");
                        ui.end_row();

                        for (row, movie) in movies.iter().enumerate() {
                            ui.label(rated_title(movie));
                            ui.label(format!("{}/10", rating_text(movie.rating())));
                            match movie.tmdb_id {
                                Some(id) => {
                                    let text = matched
                                        .get(&id)
                                        .map(|m| format!("{} ({})", m.title, year(&m.release_date)))
                                        .unwrap_or_else(|| format!("TMDB #{id}"));
                                    ui.hyperlink_to(
                                        text,
                                        format!("https://www.themoviedb.org/movie/{id}"),
                                    );
                                }
                                None if ranked_once => {
                                    ui.label(
                                        RichText::new("Not found")
                                            .color(Color32::from_rgb(230, 120, 100)),
                                    );
                                }
                                None => {
                                    ui.label(RichText::new("Not matched yet").weak());
                                }
                            }
                            match editing {
                                Some((editing_row, text)) if *editing_row == row => {
                                    let mut cancel = false;
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.add(
                                                egui::TextEdit::singleline(text)
                                                    .hint_text("themoviedb.org/movie/… link or id")
                                                    .desired_width(220.0),
                                            );
                                            let parsed = parse_tmdb_id(text);
                                            if ui
                                                .add_enabled(
                                                    parsed.is_some() && !checking,
                                                    egui::Button::new("Save"),
                                                )
                                                .on_hover_text("Checks the id on TMDB, then saves it")
                                                .clicked()
                                            {
                                                if let Some(id) = parsed {
                                                    action = Some(Action::CheckTmdbId(row, id));
                                                }
                                            }
                                            if movie.tmdb_id.is_some()
                                                && ui
                                                    .add_enabled(!checking, egui::Button::new("Clear"))
                                                    .on_hover_text(
                                                        "Forget this match and look the movie up by title again",
                                                    )
                                                    .clicked()
                                            {
                                                action = Some(Action::ClearTmdbId(row));
                                            }
                                            if ui.button("Cancel").clicked() {
                                                cancel = true;
                                            }
                                        });
                                        if !text.trim().is_empty() && parse_tmdb_id(text).is_none() {
                                            ui.label(
                                                RichText::new(
                                                    "Paste a themoviedb.org/movie/… link or just the number.",
                                                )
                                                .small()
                                                .color(Color32::from_rgb(230, 120, 100)),
                                            );
                                        }
                                    });
                                    if cancel {
                                        *editing = None;
                                    }
                                }
                                _ => {
                                    if ui.small_button("Fix").clicked() {
                                        *editing = Some((row, String::new()));
                                    }
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
        action
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        let mut settings_changed = false;
        ui.heading("Settings");
        ui.add_space(6.0);

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                section(ui, "Movie Match server", |ui| {
                    ui.label(
                        "Optional. With a server, Movie Match needs no TMDB API key: the \
                         server looks movies up with its own.",
                    );
                    ui.horizontal(|ui| {
                        settings_changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut self.settings.server_url)
                                    .hint_text("http://127.0.0.1:8765")
                                    .desired_width(320.0),
                            )
                            .changed();
                    });
                    let typed = !self.settings.server_url.trim().is_empty();
                    if typed && self.server_url().is_none() {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "Start the address with http:// or https://. Until then, the API \
                             key below is used.",
                        );
                    } else if typed {
                        ui.label(RichText::new("Using this server instead of an API key.").weak());
                    } else if let Some(url) = self.server_url() {
                        ui.label(
                            RichText::new(format!(
                                "Using {url} from MOVIE_MATCH_SERVER instead of an API key."
                            ))
                            .weak(),
                        );
                    }
                });

                section(ui, "TMDB API key", |ui| {
                    let saved = !self.settings.api_key.trim().is_empty();
                    let status = if saved {
                        "A key is saved on this computer."
                    } else if self.api_key().is_some() {
                        "Using TMDB_API_KEY from the environment."
                    } else {
                        "No key yet. Movie Match needs a free TMDB API key to look up movies."
                    };
                    ui.label(status);
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.key_input)
                                .password(true)
                                .hint_text("Paste your API key")
                                .desired_width(320.0),
                        );
                        let has_input = !self.key_input.trim().is_empty();
                        if ui
                            .add_enabled(has_input, egui::Button::new("Save key"))
                            .clicked()
                        {
                            self.settings.api_key = self.key_input.trim().to_string();
                            self.key_input.clear();
                            settings_changed = true;
                            self.notice = Some(Notice::Info("API key saved.".into()));
                            if self.ratings.is_some() && self.ranking.is_none() {
                                action = Some(Action::Rank { refresh: false });
                            }
                        }
                        if saved && ui.button("Remove saved key").clicked() {
                            self.settings.api_key.clear();
                            settings_changed = true;
                        }
                    });
                    ui.hyperlink_to("Get a free key from TMDB", API_KEY_URL);
                    ui.label(
                        RichText::new(
                            "Use the \"API Key\", not the \"Read Access Token\". For the \
                             application URL, your GitHub repository link or http://localhost is fine.",
                        )
                        .weak(),
                    );
                });

                section(ui, "Ranking", |ui| {
                    ui.horizontal(|ui| {
                        settings_changed |= ui
                            .add(egui::DragValue::new(&mut self.settings.upcoming_pages).range(1..=10))
                            .changed();
                        ui.label("pages of upcoming releases to rate (20 movies each)");
                    });
                    ui.horizontal(|ui| {
                        settings_changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut self.settings.upcoming_region)
                                    .char_limit(2)
                                    .desired_width(28.0),
                            )
                            .changed();
                        ui.label("country: Upcoming shows only films in cinemas there (blank for all)");
                    });
                    let region = self.settings.upcoming_region.trim();
                    if !region.is_empty() && Region::parse(region).is_none() {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "Use a two-letter country code such as US or GB. Until then, \
                             Upcoming shows films from every country.",
                        );
                    }
                    ui.horizontal(|ui| {
                        settings_changed |= ui
                            .add(egui::DragValue::new(&mut self.settings.recent_pages).range(0..=10))
                            .changed();
                        ui.label("pages of recent releases to rate per 3 months");
                    });
                    ui.horizontal(|ui| {
                        settings_changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.settings.recent_months)
                                    .range(1..=24)
                                    .suffix(" months"),
                            )
                            .changed();
                        ui.label("how far back Recent reaches");
                    });
                    ui.horizontal(|ui| {
                        settings_changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.settings.reference_pages)
                                    .range(0..=20),
                            )
                            .changed();
                        ui.label("pages of popular movies used to judge how rare each feature is");
                    });
                    settings_changed |= ui
                        .checkbox(
                            &mut self.settings.use_cache,
                            "Reuse downloaded TMDB data (lists for a day, details for 3 days)",
                        )
                        .changed();
                    ui.label(RichText::new("Changes apply the next time you reload.").weak());
                });

                section(ui, "Storage", |ui| {
                    ui.label("Your ratings, settings and downloaded movie data are kept in:");
                    ui.label(RichText::new(self.store.dir().display().to_string()).monospace());
                    if ui.button("Clear downloaded data").clicked() {
                        self.notice = Some(match self.store.clear_cache() {
                            Ok(()) => Notice::Info("Downloaded data cleared.".into()),
                            Err(err) => Notice::Error(format!("{err:#}")),
                        });
                    }
                });

                section(ui, "About", |ui| {
                    ui.label(format!(
                        "Movie Match {} · movie data from TMDB. This product uses the TMDB API \
                         but is not endorsed or certified by TMDB.",
                        env!("CARGO_PKG_VERSION")
                    ));
                });
            });

        if settings_changed {
            self.save_settings();
        }
        action
    }
}

impl eframe::App for MovieMatchApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_jobs(&ctx);

        let dropped = ui.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.import_file(&ctx, &path);
        }

        egui::Panel::top("top_bar").show(ui, |ui| self.top_bar(ui));
        let action = egui::CentralPanel::default_margins()
            .show(ui, |ui| {
                self.notice_banner(ui);
                match self.tab {
                    Tab::Upcoming => self.list_tab(ui, false),
                    Tab::Recent => self.list_tab(ui, true),
                    Tab::Explain => self.explain_tab(ui),
                    Tab::Ratings => self.ratings_tab(ui),
                    Tab::Settings => self.settings_tab(ui),
                }
            })
            .inner;
        if let Some(action) = action {
            self.run(&ctx, action);
        }
    }
}

// ---- Widgets -------------------------------------------------------------------

fn section(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).strong().size(16.0));
            ui.add_space(4.0);
            add_contents(ui);
        });
    ui.add_space(10.0);
}

/// One ranked movie. Returns `true` when "Why this score?" was clicked.
/// One ranked movie. With `recent`, the audience score is always mentioned (or that
/// there isn't one yet); upcoming films only show it when early votes exist.
fn movie_card(ui: &mut egui::Ui, position: usize, ranked: &Ranked) -> bool {
    let mut explain = false;
    egui::Frame::group(ui.style())
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                poster(ui, &ranked.movie);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{position}.")).weak());
                        ui.label(RichText::new(&ranked.movie.title).strong().size(17.0));
                        ui.label(RichText::new(release_text(&ranked.movie)).weak());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            score_badge(ui, ranked.score);
                        });
                    });
                    let breakdown = &ranked.breakdown;
                    ui.horizontal(|ui| {
                        basis_chip(ui, breakdown);
                        if breakdown.is_unusual() {
                            unusual_chip(ui);
                        }
                    });
                    let basis = breakdown.basis();
                    if let Some(audience) = &breakdown.audience {
                        if basis != Basis::Estimate {
                            ui.label(audience_text(audience));
                        }
                    }
                    if let Some((title, rating)) = &ranked.closest_liked {
                        let because = if ranked.reasons.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", ranked.reasons.join(", "))
                        };
                        ui.label(format!(
                            "Like {title} (you rated {}/10){because}",
                            rating_text(*rating)
                        ));
                    }
                    if basis != Basis::Audience {
                        if let Some(text) =
                            breakdown.track_record.as_ref().and_then(track_record_text)
                        {
                            ui.label(text);
                        }
                    }
                    if let Some((title, rating)) = &ranked.closest_disliked {
                        ui.label(
                            RichText::new(format!(
                                "But resembles {title}, which you rated {}/10",
                                rating_text(*rating)
                            ))
                            .color(Color32::from_rgb(230, 120, 100)),
                        );
                    }
                    if let Some(overview) = ranked.movie.overview.as_deref() {
                        if !overview.is_empty() {
                            ui.label(RichText::new(shorten(overview, 240)).weak());
                        }
                    }
                    if ui.small_button("Why this score?").clicked() {
                        explain = true;
                    }
                });
            });
        });
    ui.add_space(6.0);
    explain
}

fn explanation_view(ui: &mut egui::Ui, movie: &MovieDetails, explanation: &Explanation) {
    ui.horizontal_top(|ui| {
        poster(ui, movie);
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(&movie.title).strong().size(20.0));
            ui.label(RichText::new(release_text(movie)).weak());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                score_badge(ui, explanation.score);
                ui.label(format!(
                    "Predicted rating {:.1}/10",
                    explanation.predicted_rating
                ));
            });
            if let Some(overview) = movie.overview.as_deref() {
                if !overview.is_empty() {
                    ui.label(RichText::new(overview).weak());
                }
            }
        });
    });
    ui.add_space(10.0);
    section(ui, "How the score was predicted", |ui| {
        breakdown_view(ui, &explanation.breakdown);
    });
    section(ui, "Similarity rating", |ui| {
        if explanation.neighbours.is_empty() {
            ui.label(format!(
                "Nothing you've rated is similar, so the similarity rating is your overall \
                 average ({:.1}/10).",
                explanation.mean_rating
            ));
        } else {
            ui.label(format!(
                "Your ratings of the {} movies below average {:.1}/10, with closer movies \
                 counting far more: weight = (similarity / closest similarity)².",
                explanation.neighbours.len(),
                explanation.neighbour_rating
            ));
            ui.label(format!(
                "Your overall average ({:.1}/10) makes up {:.0}% of the similarity rating; it \
                 counts more when few movies are similar.",
                explanation.mean_rating,
                explanation.overall_share * 100.0
            ));
        }
    });
    section(ui, "Most similar movies you've rated", |ui| {
        if explanation.neighbours.is_empty() {
            ui.label("None share any features with this movie.");
        }
        for (i, n) in explanation.neighbours.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}. {}", i + 1, n.title)).strong());
                ui.label(format!("you rated {}/10", rating_text(n.rating)));
                ui.label(
                    RichText::new(format!(
                        "similarity {:.3} · weight {:.2}",
                        n.similarity, n.weight
                    ))
                    .weak(),
                );
            });
            if !n.shared.is_empty() {
                ui.label(RichText::new(format!("Shares {}", n.shared.join(", "))).weak());
            }
            ui.add_space(4.0);
        }
    });
}

/// The score's parts: the released formula (audience + the user's shift), the
/// unreleased one (similarity + track record), and how they blend.
fn breakdown_view(ui: &mut egui::Ui, breakdown: &Breakdown) {
    ui.horizontal(|ui| {
        basis_chip(ui, breakdown);
        if breakdown.is_unusual() {
            unusual_chip(ui);
        }
    });
    let share = breakdown.audience_share;
    ui.add(
        egui::ProgressBar::new(share)
            .desired_width(320.0)
            .text(format!(
                "{:.0}% audience · {:.0}% your similar movies",
                share * 100.0,
                (1.0 - share) * 100.0
            )),
    );
    ui.add_space(6.0);

    if let Some(audience) = &breakdown.audience {
        ui.label(RichText::new("From the audience").strong());
        ui.label(format!(
            "TMDB audience {:.1}/10 from {} votes",
            audience.score,
            thousands(audience.votes)
        ));
        let shift = &audience.shift;
        let reasons: Vec<String> = shift
            .reasons
            .iter()
            .map(|(label, value)| format!("{label} {value:+.1}"))
            .collect();
        ui.label(format!(
            "{:+.1} for you: you usually rate {:+.1} vs the audience{}{}",
            shift.total(),
            shift.average,
            if reasons.is_empty() { "" } else { "; " },
            reasons.join(", ")
        ));
        ui.label(format!("= {:.1}", audience.rating));
        ui.add_space(6.0);
    }

    ui.label(RichText::new("From your similar movies").strong());
    ui.label(format!(
        "Similarity rating {:.1}/10 from {} movies you've rated (below)",
        breakdown.similarity, breakdown.neighbours
    ));
    match &breakdown.track_record {
        Some(t) => {
            ui.label(format!(
                "{:+.1} for its cast and crew: their earlier films average {:.1}, a typical \
                 film {:.1}",
                t.nudge, t.estimate, t.typical
            ));
            for person in &t.people {
                ui.label(
                    RichText::new(format!(
                        "  {} {}: {:.1} over {} earlier films",
                        role_word(person.role),
                        person.name,
                        person.record,
                        person.films
                    ))
                    .weak(),
                );
            }
        }
        None => {
            ui.label(
                RichText::new("No track record: its cast and crew have no earlier films").weak(),
            );
        }
    }
    ui.label(format!("= {:.1}", breakdown.unreleased));

    if breakdown.audience.is_some() {
        ui.add_space(6.0);
        ui.label(format!(
            "Blended by votes: {:.0}% × {:.1} + {:.0}% × {:.1} = {:.1}",
            share * 100.0,
            breakdown.audience.as_ref().map_or(0.0, |a| a.rating),
            (1.0 - share) * 100.0,
            breakdown.unreleased,
            breakdown.rating
        ));
    }
    if breakdown.is_unusual() {
        ui.add_space(4.0);
        ui.label(RichText::new(UNUSUAL_HINT).weak());
    }
}

const UNUSUAL_HINT: &str = "Little you've rated is like this film, so the similar-movies part \
     says less than usual. Treat the score as a rough guess.";

/// What the score mostly rests on: an estimate, early votes or the audience.
fn basis_chip(ui: &mut egui::Ui, breakdown: &Breakdown) {
    let votes = breakdown.audience.as_ref().map_or(0, |a| a.votes);
    let (text, hover) = match breakdown.basis() {
        Basis::Estimate => (
            "Estimate".to_string(),
            "No or very few audience ratings yet: predicted from the movies you've rated and \
             its cast and crew's earlier films",
        ),
        Basis::EarlyAudience => (
            format!("Early audience · {} votes", thousands(votes)),
            "Partly from its first audience ratings, partly estimated",
        ),
        Basis::Audience => (
            format!("Based on {} audience ratings", thousands(votes)),
            "Mostly its audience score, adjusted for how you usually differ from the audience",
        ),
    };
    chip(ui, &text, Color32::from_rgb(70, 110, 160)).on_hover_text(hover);
}

fn unusual_chip(ui: &mut egui::Ui) {
    chip(
        ui,
        "Not your usual kind of film",
        Color32::from_rgb(150, 100, 40),
    )
    .on_hover_text(UNUSUAL_HINT);
}

fn chip(ui: &mut egui::Ui, text: &str, fill: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(Color32::WHITE));
        })
        .response
}

/// "Audience 7.9, +0.5 for you: genre Science Fiction +0.3, director … +0.2"
fn audience_text(audience: &AudienceView) -> String {
    let reasons: Vec<String> = audience
        .shift
        .reasons
        .iter()
        .map(|(label, value)| format!("{label} {value:+.1}"))
        .collect();
    let because = if reasons.is_empty() {
        String::new()
    } else {
        format!(": {}", reasons.join(", "))
    };
    format!(
        "Audience {:.1}, {:+.1} for you{because}",
        audience.score,
        audience.shift.total()
    )
}

/// "Director Denis Villeneuve's earlier films average 7.8 (+0.4)", or `None` when
/// nobody is named.
fn track_record_text(track_record: &TrackRecordNudge) -> Option<String> {
    let mut people = track_record.people.iter();
    let first = people.next()?;
    let mut text = format!(
        "{} {}'s earlier films average {:.1}",
        capitalized(role_word(first.role)),
        first.name,
        first.record
    );
    for person in people {
        text += &format!(
            "; {} {}'s {:.1}",
            role_word(person.role),
            person.name,
            person.record
        );
    }
    Some(format!("{text} ({:+.1})", track_record.nudge))
}

fn role_word(role: Role) -> &'static str {
    match role {
        Role::Director => "director",
        Role::Writer => "writer",
        Role::Cast => "actor",
    }
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map_or(String::new(), |c| c.to_uppercase().chain(chars).collect())
}

fn poster(ui: &mut egui::Ui, movie: &MovieDetails) {
    match movie.poster_url("w185") {
        Some(url) => {
            ui.add(
                egui::Image::new(url)
                    .fit_to_exact_size(POSTER_SIZE)
                    .corner_radius(4.0),
            );
        }
        None => {
            let (rect, _) = ui.allocate_exact_size(POSTER_SIZE, egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 4.0, ui.visuals().faint_bg_color);
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "No poster",
                FontId::proportional(11.0),
                ui.visuals().weak_text_color(),
            );
        }
    }
}

fn score_badge(ui: &mut egui::Ui, score: u8) {
    let fill = match score {
        75.. => Color32::from_rgb(40, 140, 70),
        60..=74 => Color32::from_rgb(170, 125, 30),
        _ => Color32::from_rgb(110, 110, 110),
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(
                RichText::new(score.to_string())
                    .strong()
                    .size(18.0)
                    .color(Color32::WHITE),
            );
        })
        .response
        .on_hover_text(
            "Your predicted rating × 10: from the movies you've rated that are most like it \
             and its cast and crew's earlier films, or once people have seen it, from the \
             audience score adjusted for how you usually differ from it",
        );
}

fn read_ratings_file(path: &Path) -> Result<Vec<RatedMovie>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let is_toml = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"));
    if is_toml {
        profile::parse_rated_movies(&text)
    } else {
        import::parse_letterboxd_csv(&text)
    }
}

/// A TMDB movie id from a bare number or a movie link like
/// `themoviedb.org/movie/603-the-matrix`. Anything else, such as a TV link
/// (`/tv/1399`), is rejected rather than guessed at.
fn parse_tmdb_id(text: &str) -> Option<u64> {
    let text = text.trim();
    let digits: String = match text.rsplit_once("/movie/") {
        Some((_, tail)) => tail.chars().take_while(char::is_ascii_digit).collect(),
        None if text.chars().all(|c| c.is_ascii_digit()) => text.to_string(),
        None => return None,
    };
    digits.parse().ok().filter(|id| *id > 0)
}

fn rated_title(movie: &RatedMovie) -> String {
    match movie.year {
        Some(year) => format!("{} ({year})", movie.label()),
        None => movie.label(),
    }
}

fn release_text(movie: &MovieDetails) -> String {
    if movie.release_date.is_empty() {
        "release date unknown".to_string()
    } else {
        movie.release_date.clone()
    }
}

fn year(release_date: &str) -> &str {
    release_date.get(..4).unwrap_or("unknown year")
}

/// `9` for whole ratings, `7.5` otherwise.
fn rating_text(rating: f32) -> String {
    if rating.fract() == 0.0 {
        format!("{rating:.0}")
    } else {
        format!("{rating:.1}")
    }
}

/// `1234567` as `1,234,567`.
fn thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `text` cut to about `max_chars` characters at a word boundary.
fn shorten(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{cut}…")
}

/// Writes the TMDB ids a ranking resolved onto the ratings it was ranked from (one
/// id per rating, in order). Returns whether anything changed. Ids for a list of a
/// different length can't belong to these ratings, so they're ignored.
fn apply_resolved_ids(movies: &mut [RatedMovie], ids: &[Option<u64>]) -> bool {
    if movies.len() != ids.len() {
        return false;
    }
    let mut changed = false;
    for (movie, id) in movies.iter_mut().zip(ids) {
        if movie.tmdb_id != *id {
            movie.tmdb_id = *id;
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::{apply_resolved_ids, parse_tmdb_id, shorten, thousands};
    use movie_match::profile::RatedMovie;

    fn rated(title: &str, tmdb_id: Option<u64>) -> RatedMovie {
        RatedMovie {
            title: Some(title.into()),
            tmdb_id,
            ..RatedMovie::default()
        }
    }

    #[test]
    fn resolved_ids_are_saved_in_order() {
        let mut movies = vec![rated("Alien", None), rated("Heat", Some(949))];
        assert!(apply_resolved_ids(&mut movies, &[Some(348), Some(949)]));
        assert_eq!(movies[0].tmdb_id, Some(348));
        assert_eq!(movies[1].tmdb_id, Some(949));
        // Nothing new: nothing to save.
        assert!(!apply_resolved_ids(&mut movies, &[Some(348), Some(949)]));
        // An id TMDB no longer has comes back as `None` and is cleared.
        assert!(apply_resolved_ids(&mut movies, &[None, Some(949)]));
        assert_eq!(movies[0].tmdb_id, None);
    }

    #[test]
    fn ids_for_a_different_list_are_ignored() {
        let mut movies = vec![rated("Alien", None), rated("Heat", None)];
        assert!(!apply_resolved_ids(&mut movies, &[Some(603)]));
        assert!(!apply_resolved_ids(
            &mut movies,
            &[Some(603), Some(1), Some(2)]
        ));
        assert!(movies.iter().all(|m| m.tmdb_id.is_none()));
    }

    #[test]
    fn vote_counts_get_thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(2300), "2,300");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[test]
    fn tmdb_ids_come_from_links_or_numbers() {
        assert_eq!(parse_tmdb_id("603"), Some(603));
        assert_eq!(
            parse_tmdb_id("https://www.themoviedb.org/movie/603-the-matrix"),
            Some(603)
        );
        assert_eq!(parse_tmdb_id(" 27205 "), Some(27205));
        assert_eq!(parse_tmdb_id("the matrix"), None);
        assert_eq!(parse_tmdb_id("0"), None);
    }

    #[test]
    fn non_movie_links_and_stray_text_are_rejected() {
        // A TV show's id would point at an unrelated movie.
        assert_eq!(parse_tmdb_id("https://www.themoviedb.org/tv/1399"), None);
        assert_eq!(parse_tmdb_id("https://www.themoviedb.org/person/287"), None);
        assert_eq!(parse_tmdb_id("603abc"), None);
        assert_eq!(parse_tmdb_id(""), None);
    }

    #[test]
    fn shorten_cuts_at_a_word() {
        assert_eq!(shorten("short", 10), "short");
        assert_eq!(shorten("one two three four", 9), "one two…");
    }
}
