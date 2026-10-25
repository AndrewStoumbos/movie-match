# movie-match

Rust CLI that rates upcoming movie releases (from TMDB) by how well they match
movies the user likes. See README.md for the roadmap.

## Layout
- `src/main.rs`   CLI entry point (clap). Keep it thin; logic lives in the library.
- `src/lib.rs`    library root
- `src/config.rs` settings from env / `.env` (`TMDB_API_KEY`)
- `src/profile.rs` liked-movies file loading and `TasteProfile`
- `src/scoring.rs` `score(profile, movie) -> u8` (0-100)
- `src/tmdb/`     TMDB API client (`mod.rs`) and response types (`models.rs`)
- `tests/`        integration tests

## Commands
- Build: `cargo build`
- Format: `cargo fmt --all` (CI checks `cargo fmt --all -- --check`)
- Lint: `cargo clippy --all-targets -- -D warnings`
- Test: `cargo test --all`

## Rules
- Never print, log, or include the TMDB API key in errors or test fixtures.
  Don't commit `.env` or `data/liked_movies.toml`.
- Tests must not hit the network. Build `Movie` values directly (see `tests/scoring.rs`)
  or use JSON fixtures for TMDB responses.
- Keep scoring functions pure: no I/O, no randomness.
- Use `anyhow::Result` with `.context(...)` for errors; no `unwrap()`/`expect()` on
  file, env or network data.
- Default branch is `master`.
