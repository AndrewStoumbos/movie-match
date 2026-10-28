# movie-match

Tell it the movies you like, and it rates upcoming releases by how likely you are to enjoy them.

Written in Rust. Movie data comes from [TMDB](https://www.themoviedb.org/).

## Status

Working CLI and desktop app. It matches your rated movies to TMDB, fetches upcoming
releases, and ranks them with field-weighted TF-IDF similarity, explaining each match
(see [How scoring works](#how-scoring-works)). TMDB data is cached between runs.

## Desktop app

A Windows/macOS/Linux app with the same features as the CLI, no command line needed.

1. Run it: download `movie-match.exe` from a GitHub release, or build it yourself:

   ```bash
   cargo run --release -p movie-match-app
   ```

2. **Settings:** paste your free TMDB API key (https://www.themoviedb.org/settings/api).
   A `TMDB_API_KEY` in `.env` works too.
3. **Import:** choose your Letterboxd `ratings.csv` (Letterboxd → Settings → Import &
   Export → Export your data), or drag it onto the window. Star ratings are doubled to
   the 1–10 scale. A movie-match `.toml` ratings file also works, and so does
   `movie-match.exe path\to\ratings.csv` (or "Open with").

Screens:
- **Upcoming:** upcoming releases ranked for you, with posters, scores, the closest
  movie you rated and why. Filter by title, show the top 20/50/all, **Reload** (reuse
  downloads) or **Re-download** (fresh data from TMDB).
- **Explain a movie:** any title, upcoming or not, with its predicted rating and the
  rated movies behind it (the CLI's `--explain`).
- **My ratings:** what each rating matched on TMDB. **Fix** a wrong or missing match by
  pasting the TMDB link or id.
- **Settings:** API key, how many pages of upcoming and popular movies to use, whether
  to reuse downloads, and clearing downloaded data.

Everything is kept on your computer in the app-data folder (`%APPDATA%\movie-match\data`
on Windows): `settings.json` (including the API key), `ratings.json` (your imported
ratings, with the TMDB ids found so later starts skip the title searches) and `cache/`.

To make a downloadable build, `cargo build --release -p movie-match-app` produces a single
`target/release/movie-match.exe`. Pushing a tag like `v0.1.0` runs
`.github/workflows/release.yml`, which builds it on Windows and attaches it to a GitHub
release. The exe isn't code-signed, so Windows SmartScreen or Smart App Control may warn
before the first run.

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
   Every rating counts: a release similar to movies you rated highly scores high, and
   one similar to movies you rated low scores low. The example file shows every option.

4. Run it:

   ```bash
   cargo run
   ```

   Options: `--pages 3` rates more upcoming releases (20 per page),
   `--reference-pages 10` uses more popular movies to judge how rare each feature is,
   `-n 50` shows more results. Downloads are cached in `data/cache/` (upcoming and
   popular lists for a day, movie details for 3 days); `--refresh` re-downloads and
   `--no-cache` skips the cache. `--explain "By Any Means"` shows how one movie's score
   was worked out: its 20 nearest rated movies with similarity, weight, your rating
   and shared features (any title TMDB can find works). See `cargo run -- --help`.

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
2. **Field weights.** Each field has an importance: genre 2.0, keyword 1.5, director 3.0,
   writer 2.0, lead actor 2.0 (the n-th billed actor gets 2.0 / n). TMDB's mood tags
   ("aggressive", "farcical", …, see `TONE_KEYWORDS`) count only 0.4, since they describe
   feel rather than content. See `FieldWeights`.
3. **Rarity (IDF).** Each token is also weighted by how rare it is across the shared
   catalog (the upcoming releases plus a few pages of popular movies), using
   `ln((1 + N) / (1 + df)) + 1`. Sharing a director counts far more than sharing "Drama".
4. **Similarity rating.** Every rated movie is kept as its own vector. For each release
   separately, the 20 rated movies most similar to it are picked by similarity alone,
   whatever you rated them, so a sci-fi release is judged by your sci-fi-like ratings
   and a drama by your drama-like ones. Each neighbour's **weight** is its similarity
   divided by the closest one's, **squared**: the closest counts 1.0, one half as
   similar counts 0.25 (raw similarities between different films are tiny, often under
   0.1, so only their ratios matter). A low rating counts as much as an equally similar
   high one. Rated movies tied for the
   last of the 20 places (say 60 dramas a release shares only "Drama" with) are all
   kept and split the places left, so the order of your ratings file never matters.

   The similarity rating is the weighted average of those neighbours' ratings, blended
   with your overall average only by **how many** neighbours there are:
   `(n × weighted average + 5 × your overall average) / (n + 5)`. With the full 20
   neighbours your overall average is a fifth of it; with one stray overlap it's most
   of it, so one film can't decide a score; a release sharing nothing gets exactly
   your average.
5. **Track record.** A film's likely audience score is estimated from its directors',
   writers' and top 3 actors' **earlier** films (`quality.rs`), and the similarity
   rating is nudged by how far that sits from the typical film's:
   `+ 1.0 × (estimate − typical)`. This is the **unreleased** formula.
6. **Audience and your shift.** Once a film has TMDB audience votes, the **released**
   formula is its audience score plus your usual difference from the audience for films
   like it: your average difference plus a shift per genre, director, writer, actor and
   keyword, learned from your rated films by ridge regression (`shift.rs`). The raw
   audience score is never used alone: people rate on different scales.
7. **Blend.** The score is `share × released + (1 − share) × unreleased` with
   `share = votes / (votes + 100)`, so a festival film with 10 votes is mostly the
   unreleased formula and one with 5,000 almost all the released one. Tested by
   leave-one-out on MovieLens (610 users), this misses by 1.26 points on average
   against 1.36 for similarity alone, and orders each user's films about twice as well.
   `--explain` prints the breakdown. Constants live in `scoring.rs`.
8. **Explanation.** The output names the most similar liked movie and the three shared
   features that contributed most, plus the most similar disliked movie among the 20.

The upcoming and popular lists and the rarity table are the same for every user, so
they're built once (`Catalog`) and only the rated movies are per user. Movies you've
rated are never recommended back to you.

A score is your predicted rating out of 100, so it's on the same scale as your own
ratings: if you rate most films 7+, most scores will be 70+ too. Read them as a
ranking against each other.

## Project layout

```
.
├── Cargo.toml                  # library + CLI crate, and the workspace
├── app/                        # desktop app crate (egui), builds movie-match.exe
│   ├── assets/                 # icon.png (window), icon.ico (exe), make-icon.ps1
│   ├── build.rs                # embeds icon.ico in the exe on Windows
│   └── src/
│       ├── main.rs             # window setup
│       ├── app.rs              # state and screens
│       ├── jobs.rs             # runs library calls in the background
│       └── store.rs            # settings, ratings and cache in the app-data folder
├── src/
│   ├── main.rs                 # CLI entry point
│   ├── lib.rs                  # library root (modules below)
│   ├── config.rs               # settings from env / .env
│   ├── import.rs               # Letterboxd ratings.csv import
│   ├── profile.rs              # ratings file + taste profile (liked / disliked)
│   ├── features.rs             # movie -> field-weighted feature tokens + labels
│   ├── tfidf.rs                # IDF table, vectors, cosine similarity
│   ├── scoring.rs              # scores, explanations and ranking (pure)
│   ├── shift.rs                # your per-feature difference from the audience (pure)
│   ├── cache.rs                # JSON file cache with expiry
│   ├── fetch.rs                # TMDB client + cached movie details
│   ├── catalog.rs              # shared upcoming/popular movies and rarity table
│   ├── recommend.rs            # per user: resolve ratings, rank the catalog
│   └── tmdb/
│       ├── mod.rs              # TMDB API client
│       └── models.rs           # TMDB response types
├── tests/
│   ├── scoring.rs              # scoring, dislikes, explanations, ranking
│   ├── two_case.rs             # audience shift, track-record nudge, vote blend
│   ├── features.rs             # feature extraction and TF-IDF
│   ├── profile.rs              # ratings file parsing and validation
│   ├── cache.rs                # disk cache behaviour
│   ├── import.rs               # Letterboxd CSV parsing
│   ├── recommend.rs            # explain lookups against a local TMDB stand-in
│   ├── tmdb_errors.rs          # TMDB errors never reveal the API key
│   ├── tmdb_proxy.rs           # test clients bypass proxy env vars
│   ├── common/                 # test movie builders, local TMDB stand-in server
│   └── fixtures/               # sample TMDB JSON responses
├── data/
│   └── liked_movies.example.toml
├── .github/workflows/ci.yml    # build, lint and test on every push
├── .github/workflows/release.yml # Windows exe on every v* tag
├── .env.example
└── .gitignore
```

## Roadmap

- [x] Look up liked movies on TMDB and build a real taste profile
- [x] Score with keywords, director and cast, not just genres
- [x] Cache TMDB details and the shared catalog so repeat runs are faster
- [x] Ratings, with disliked movies lowering the score
- [x] Explain each score
- [x] Import Letterboxd ratings
- [x] Desktop app
- [x] Title search ("Explain a movie")
- [ ] Import from Trakt
- [ ] General recommendations (released movies you haven't seen)
- [ ] Tune field weights against MovieLens ratings
- [ ] Plot-summary similarity (text embeddings)
- [ ] Optional LLM pass for nuanced scores with a short explanation
- [ ] "Did you like it?" feedback to improve future scores
- [ ] Web UI
