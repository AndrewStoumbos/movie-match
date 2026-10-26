# movie-match

Tell it the movies you like, and it rates upcoming releases by how likely you are to enjoy them.

Written in Rust. Movie data comes from [TMDB](https://www.themoviedb.org/).

## Status

Working CLI. It looks up your liked movies on TMDB, fetches upcoming releases, and
ranks them with field-weighted TF-IDF similarity (see [How scoring works](#how-scoring-works)).

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

   Options: `--pages 3` rates more upcoming releases (20 per page),
   `--reference-pages 10` uses more popular movies to judge how rare each feature is,
   `-n 50` shows more results. See `cargo run -- --help`.

Run the tests with `cargo test`.

## How scoring works

1. **Features.** Each movie becomes a set of labeled tokens from TMDB: genres, keywords,
   directors, writers and the top 5 billed actors (`genre:878`, `kw:4379`, `dir:137427`, …).
2. **Field weights.** Each field has an importance: genre 1.0, keyword 1.5, director 3.0,
   writer 2.0, lead actor 2.0 (the n-th billed actor gets 2.0 / n). See `FieldWeights`.
3. **Rarity (IDF).** Each token is also weighted by how rare it is across a reference set
   (your liked movies, the upcoming releases and a few pages of popular movies), using
   `ln((1 + N) / (1 + df)) + 1`. Sharing a director counts far more than sharing "Drama".
4. **Similarity.** Every liked movie is kept as its own vector. A release's score is its
   cosine similarity (0–100) to the **single closest** liked movie, so eclectic tastes
   don't get averaged into mush. The output names that closest movie.

Scores are best read as a ranking: a score of 30 can already be a strong match, because
two different movies rarely share many keywords and people.

## Project layout

```
.
├── Cargo.toml                  # crate metadata and dependencies
├── src/
│   ├── main.rs                 # CLI entry point
│   ├── lib.rs                  # library root (modules below)
│   ├── config.rs               # settings from env / .env
│   ├── profile.rs              # liked movies + taste profile
│   ├── features.rs             # movie -> field-weighted feature tokens
│   ├── tfidf.rs                # IDF table, vectors, cosine similarity
│   ├── scoring.rs              # rates a movie against a profile
│   ├── recommend.rs            # fetches from TMDB and ranks upcoming releases
│   └── tmdb/
│       ├── mod.rs              # TMDB API client
│       └── models.rs           # TMDB response types
├── tests/
│   ├── scoring.rs              # scoring behaviour
│   ├── features.rs             # feature extraction and TF-IDF
│   ├── tmdb_errors.rs          # TMDB errors never reveal the API key
│   ├── tmdb_proxy.rs           # test clients bypass proxy env vars
│   ├── common/                 # test movie builders, local TMDB stand-in server
│   └── fixtures/               # sample TMDB JSON responses
├── data/
│   └── liked_movies.example.toml
├── .github/workflows/ci.yml    # build, lint and test on every push
├── .env.example
└── .gitignore
```

## Roadmap

- [x] Look up liked movies on TMDB and build a real taste profile
- [x] Score with keywords, director and cast, not just genres
- [ ] Cache TMDB details and the IDF table so repeat runs are faster
- [ ] Disliked movies that lower the score
- [ ] Tune field weights against MovieLens ratings
- [ ] Plot-summary similarity (text embeddings)
- [ ] Optional LLM pass for nuanced scores with a short explanation
- [ ] "Did you like it?" feedback to improve future scores
- [ ] Web UI
