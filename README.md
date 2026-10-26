# movie-match

Tell it the movies you like, and it rates upcoming releases by how likely you are to enjoy them.

Written in Rust. Movie data comes from [TMDB](https://www.themoviedb.org/).

## Status

Working CLI. It matches your rated movies to TMDB, fetches upcoming releases, and
ranks them with field-weighted TF-IDF similarity, explaining each match
(see [How scoring works](#how-scoring-works)). TMDB data is cached between runs.

## Getting started

1. Install Rust: https://rustup.rs
2. Get a free TMDB API key: https://www.themoviedb.org/settings/api
3. Set up your local files:

   ```bash
   cp .env.example .env                                   # then paste your API key in
   cp data/liked_movies.example.toml data/liked_movies.toml   # then list your movies
   ```

   Each entry needs a `title` (plus `year` to pick the right one) or an exact `tmdb_id`,
   and an optional `rating` from 0.5 to 10. Unrated entries count as 10/10 favourites.
   7 and up counts as liked, 4 and down as disliked, and anything in between only
   marks the movie as seen. The example file shows every option.

4. Run it:

   ```bash
   cargo run
   ```

   Options: `--pages 3` rates more upcoming releases (20 per page),
   `--reference-pages 10` uses more popular movies to judge how rare each feature is,
   `-n 50` shows more results. Downloads are cached in `data/cache/` (upcoming and
   popular lists for a day, movie details for 3 days); `--refresh` re-downloads and
   `--no-cache` skips the cache. See `cargo run -- --help`.

   Sample output:

   ```
     72  Some Upcoming Movie [2026-12-18]
          like Arrival (10/10): director Denis Villeneuve, keyword "alien contact"
          but resembles Some Movie, which you rated 3/10
   ```

Run the tests with `cargo test`.

## How scoring works

1. **Features.** Each movie becomes a set of labeled tokens from TMDB: genres, keywords,
   directors, writers and the top 5 billed actors (`genre:878`, `kw:4379`, `dir:137427`, …).
2. **Field weights.** Each field has an importance: genre 1.0, keyword 1.5, director 3.0,
   writer 2.0, lead actor 2.0 (the n-th billed actor gets 2.0 / n). See `FieldWeights`.
3. **Rarity (IDF).** Each token is also weighted by how rare it is across the shared
   catalog (the upcoming releases plus a few pages of popular movies), using
   `ln((1 + N) / (1 + df)) + 1`. Sharing a director counts far more than sharing "Drama".
4. **Similarity.** Every liked and disliked movie is kept as its own vector, so eclectic
   tastes don't get averaged into mush. A release's score is
   `similarity to the closest liked movie × (its rating / 10)`
   `− 0.5 × similarity to the closest disliked movie × dislike strength`, clamped to 0–100.
   Dislike strength runs from 0.25 at 4/10 to 1.0 at 1/10.
5. **Explanation.** The output names the closest liked movie and the three shared
   features that contributed most, plus the disliked movie that lowered the score.

The upcoming and popular lists and the rarity table are the same for every user, so
they're built once (`Catalog`) and only the rated movies are per user. Movies you've
rated are never recommended back to you.

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
│   ├── profile.rs              # ratings file + taste profile (liked / disliked)
│   ├── features.rs             # movie -> field-weighted feature tokens + labels
│   ├── tfidf.rs                # IDF table, vectors, cosine similarity
│   ├── scoring.rs              # scores, explanations and ranking (pure)
│   ├── cache.rs                # JSON file cache with expiry
│   ├── fetch.rs                # TMDB client + cached movie details
│   ├── catalog.rs              # shared upcoming/popular movies and rarity table
│   ├── recommend.rs            # per user: resolve ratings, rank the catalog
│   └── tmdb/
│       ├── mod.rs              # TMDB API client
│       └── models.rs           # TMDB response types
├── tests/
│   ├── scoring.rs              # scoring, dislikes, explanations, ranking
│   ├── features.rs             # feature extraction and TF-IDF
│   ├── profile.rs              # ratings file parsing and validation
│   ├── cache.rs                # disk cache behaviour
│   ├── common/                 # test movie builders
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
- [x] Cache TMDB details and the shared catalog so repeat runs are faster
- [x] Ratings, with disliked movies lowering the score
- [x] Explain each score
- [ ] Import ratings from Trakt and Letterboxd
- [ ] General recommendations and title search
- [ ] Tune field weights against MovieLens ratings
- [ ] Plot-summary similarity (text embeddings)
- [ ] Optional LLM pass for nuanced scores with a short explanation
- [ ] "Did you like it?" feedback to improve future scores
- [ ] Web UI
