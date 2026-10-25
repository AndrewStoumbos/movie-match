# movie-match

Tell it the movies you like, and it rates upcoming releases by how likely you are to enjoy them.

Written in Rust. Movie data comes from [TMDB](https://www.themoviedb.org/).

## Status

Early skeleton. It can load your liked movies and fetch upcoming releases from TMDB;
taste profiles and scoring are still basic (genre overlap only).

## Getting started

1. Install Rust: https://rustup.rs
2. Get a free TMDB API key: https://www.themoviedb.org/settings/api
3. Set up your local files:

   ```bash
   cp .env.example .env                                   # then paste your API key in
   cp data/liked_movies.example.toml data/liked_movies.toml   # then list your movies
   ```

4. Run it:

   ```bash
   cargo run
   ```

Run the tests with `cargo test`.

## Project layout

```
.
├── Cargo.toml                  # crate metadata and dependencies
├── src/
│   ├── main.rs                 # CLI entry point
│   ├── lib.rs                  # library root (modules below)
│   ├── config.rs               # settings from env / .env
│   ├── profile.rs              # liked movies + taste profile
│   ├── scoring.rs              # rates a movie against a profile
│   └── tmdb/
│       ├── mod.rs              # TMDB API client
│       └── models.rs           # TMDB response types
├── tests/
│   └── scoring.rs              # integration tests
├── data/
│   └── liked_movies.example.toml
├── .github/workflows/ci.yml    # build, lint and test on every push
├── .env.example
└── .gitignore
```

## Roadmap

- [ ] Look up liked movies on TMDB and build a real taste profile
- [ ] Score with keywords, director and cast, not just genres
- [ ] Plot-summary similarity (text embeddings)
- [ ] Optional LLM pass for nuanced scores with a short explanation
- [ ] "Did you like it?" feedback to improve future scores
- [ ] Web UI
