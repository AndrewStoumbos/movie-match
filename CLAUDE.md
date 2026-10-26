# movie-match

Rust CLI that rates upcoming movie releases (from TMDB) by how well they match
movies the user likes. See README.md for the roadmap.

## Layout
- `src/main.rs`   CLI entry point (clap). Keep it thin; logic lives in the library.
- `src/lib.rs`    library root
- `src/config.rs` settings from env / `.env` (`TMDB_API_KEY`)
- `src/profile.rs` `RatedMovie` ratings file (title or `tmdb_id`, rating 0.5-10) and
  `TasteProfile` (every rated movie as a vector, plus the mean rating; liked >= 7 and
  disliked <= 4 only label explanations)
- `src/features.rs` `MovieDetails` -> field-weighted tokens (`FieldWeights`) and labels
- `src/tfidf.rs`  `Idf` table, unit-length `Vector`, cosine similarity
- `src/scoring.rs` `score(profile, movie) -> u8` (0-100): predicted rating × 10 from the
  movie's 20 most similar rated movies (`predict`: weights relative to the closest,
  mean rating as prior scaled by 1 - closest similarity, all weights scaled by
  confidence min(1, closest similarity / 0.08), neighbours among the user's bottom 40% of ratings
  (`TasteProfile::is_low_rating`) x 0.7). `score_detailed` (with
  reasons), `explain` (for `--explain`), `rank`. Pure.
- `src/cache.rs`  `DiskCache`: JSON files with expiry under `data/cache/` (gitignored).
  Callers save with `put_or_warn`: a failed cache write is a warning, not an error
- `src/fetch.rs`  `Fetcher`: TMDB client + cached movie details. `details` fails on any
  failed id; `available_details` (used by the catalog) skips only ids TMDB answers
  404 for (`tmdb::is_not_found`), with one summary warning, and fails on other errors
- `src/catalog.rs` `Catalog`: shared upcoming/popular movies and IDF, same for every user
- `src/recommend.rs` per-user steps: `resolve` ratings to TMDB, `rank_upcoming`
- I/O lives only in `tmdb/`, `cache.rs`, `fetch.rs`, `catalog.rs`, `recommend.rs`.
- `src/tmdb/`     TMDB API client (`mod.rs`) and response types (`models.rs`)
- `tests/`        integration tests; builders in `tests/common/`, JSON in `tests/fixtures/`

## Commands
- Build: `cargo build`
- Format: `cargo fmt --all` (CI checks `cargo fmt --all -- --check`)
- Lint: `cargo clippy --all-targets -- -D warnings`
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
