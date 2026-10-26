# movie-match

Rust CLI that rates upcoming movie releases (from TMDB) by how well they match
movies the user likes. See README.md for the roadmap.

## Layout
- `src/main.rs`   CLI entry point (clap). Keep it thin; logic lives in the library.
- `src/lib.rs`    library root
- `src/config.rs` settings from env / `.env` (`TMDB_API_KEY`)
- `src/import.rs` `parse_letterboxd_csv`: Letterboxd `ratings.csv` -> `RatedMovie` (stars x 2). Pure.
- `src/profile.rs` `RatedMovie` ratings file (title or `tmdb_id`, rating 0.5-10) and
  `TasteProfile` (every rated movie as a vector, plus the mean rating; liked >= 7 and
  disliked <= 4 only label explanations)
- `src/features.rs` `MovieDetails` -> field-weighted tokens (`FieldWeights`) and labels
- `src/tfidf.rs`  `Idf` table, unit-length `Vector`, cosine similarity
- `src/scoring.rs` `score(profile, movie) -> u8` (0-100): predicted rating × 10 from the
  movie's 20 most similar rated movies (`predict`: weight = (similarity / closest)^2
  (`SIMILARITY_POWER`), x 0.7 for neighbours among the user's bottom 40% of ratings
  (`TasteProfile::is_low_rating`); movies tied for the 20th place are all kept and
  split the places left, so file order never matters; the weighted average is blended with the overall
  average as (n x weighted + 5 x overall) / (n + 5) for n neighbours
  (`OVERALL_SHRINK`), so exactly the overall average with none). `score_detailed` (with
  reasons), `explain` (for `--explain`), `rank`. Pure.
- `src/cache.rs`  `DiskCache`: JSON files with expiry under `data/cache/` (gitignored).
  Callers save with `put_or_warn`: a failed cache write is a warning, not an error
- `src/fetch.rs`  `Fetcher`: TMDB client + cached movie details, keyed by
  `details_cache_key` (use it in tests, never a literal key). `details` fails on any
  failed id; `available_details` (used by the catalog) skips only ids TMDB answers
  404 for (`tmdb::is_not_found`), with one summary warning, and fails on other errors;
  `details_if_found` gives `None` per 404 and also fails on other errors
- `src/catalog.rs` `Catalog`: shared upcoming/popular movies and IDF, same for every user
- `src/recommend.rs` per-user steps: `resolve` ratings to TMDB (`Resolved::ids` per input,
  in order; a given id TMDB answers 404 for becomes unmatched with id `None`, so one
  stale id can't fail every run), `rank_upcoming`, `explain_movie`. Futures must stay `Send` (the app runs them
  on a multi-threaded runtime): stream over owned items, not references.
- I/O lives only in `tmdb/`, `cache.rs`, `fetch.rs`, `catalog.rs`, `recommend.rs`.
- `src/tmdb/`     TMDB API client (`mod.rs`) and response types (`models.rs`)
- `tests/`        integration tests; builders in `tests/common/`, JSON in `tests/fixtures/`
- `app/`          desktop app crate `movie-match-app` (eframe/egui), binary `movie-match`.
  `app.rs` screens, `jobs.rs` background tasks, `store.rs` files in the app-data folder
  (settings incl. API key, ratings with found TMDB ids, cache). UI only: logic goes in
  the library. Icon: `app/assets/make-icon.ps1` draws `icon.png` (window) and `icon.ico`
  (embedded in the exe by `app/build.rs` via winresource); edit the script, not the images.

## Commands
- Build: `cargo build`
- Run the desktop app: `cargo run -p movie-match-app` (release exe:
  `cargo build --release -p movie-match-app`)
- Format: `cargo fmt --all` (CI checks `cargo fmt --all -- --check`)
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Test: `cargo test --all`

## Rules
- Never print, log, or include the TMDB API key in errors or test fixtures.
  Don't commit `.env` or `data/liked_movies.toml`.
- Tests must not hit the network. Build `MovieDetails` values with the builders in
  `tests/common/` or use JSON fixtures for TMDB responses (`tests/fixtures/`).
- Keep scoring functions pure: no I/O, no randomness.
- Use `anyhow::Result` with `.context(...)` for errors; no `unwrap()`/`expect()` on
  file, env or network data.
- Default branch is `master`.
