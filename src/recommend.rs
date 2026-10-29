//! The per-user steps: resolve a user's ratings to TMDB movies, then rank the
//! shared catalog against them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};

use crate::benchmark::{self, BenchmarkReport};
use crate::catalog::{Catalog, CatalogOptions};
use crate::evaluate::{self, Evaluation, UsersReport};
use crate::features::{self, FieldWeights};
use crate::fetch::{Fetcher, CONCURRENT_REQUESTS};
use crate::movielens::UserRatings;
use crate::profile::{RatedDetails, RatedMovie, TasteProfile};
use crate::quality::{self, TRACK_RECORD_CAST};
use crate::scoring::{self, Explanation, Ranked, Signals, TrackRecords};
use crate::shift::AudienceShift;
use crate::tfidf::Idf;
use crate::tmdb::{CreditedFilm, Movie, MovieDetails};

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub catalog: CatalogOptions,
    pub weights: FieldWeights,
}

/// A user's ratings matched to TMDB movies.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub rated: Vec<RatedDetails>,
    /// Entries matched by title search, as (what was searched, what TMDB found).
    /// Worth double-checking: the top search result can be the wrong movie.
    pub searched: Vec<(String, String)>,
    /// Entries TMDB search found nothing for, or whose TMDB id TMDB doesn't have
    /// (it answered 404).
    pub unmatched: Vec<String>,
    /// The TMDB id found for each input movie, in input order (`None` if unmatched,
    /// including a given id TMDB doesn't have). Saving these lets later runs skip
    /// the title searches.
    pub ids: Vec<Option<u64>>,
}

#[derive(Debug, Clone)]
pub struct Ranking {
    pub resolved: Resolved,
    /// Upcoming releases (today or later) the user hasn't rated, best match first.
    pub ranked: Vec<Ranked>,
    /// Recent releases (see [`CatalogOptions::recent_months`]) the user hasn't
    /// rated, best match first.
    pub recent: Vec<Ranked>,
}

/// The steps of a ranking, reported as each starts (see [`rank_upcoming_reporting`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Listing upcoming, recent and popular movies (cached for a day).
    Catalog,
    /// Matching the user's ratings to TMDB.
    Ratings,
    /// Fetching the filmographies of this many cast and crew (cached for a month).
    TrackRecords {
        people: usize,
    },
    Scoring,
}

/// Ranks the catalog's upcoming and recent releases for one user (see
/// [`scoring::rank`]), with their difference from the audience and the
/// candidates' cast and crew track records.
pub async fn rank_upcoming(
    fetcher: &Fetcher,
    movies: &[RatedMovie],
    options: &Options,
) -> Result<Ranking> {
    rank_upcoming_reporting(fetcher, movies, options, |_| {}).await
}

/// [`rank_upcoming`], calling `report` as each [`Stage`] starts.
pub async fn rank_upcoming_reporting(
    fetcher: &Fetcher,
    movies: &[RatedMovie],
    options: &Options,
    report: impl Fn(Stage) + Send + Sync,
) -> Result<Ranking> {
    report(Stage::Catalog);
    let catalog = Catalog::load(fetcher, options.catalog).await?;
    report(Stage::Ratings);
    let resolved = resolve(fetcher, movies).await?;
    let profile = TasteProfile::new(
        &resolved.rated,
        catalog.idf(&options.weights),
        options.weights,
    );
    let candidates: Vec<&MovieDetails> = catalog.upcoming.iter().chain(&catalog.recent).collect();
    let signals = signals(fetcher, &resolved, &candidates, options.weights, &report).await?;
    report(Stage::Scoring);
    let ranked = scoring::rank(&profile, &signals, &catalog.upcoming);
    let recent = scoring::rank(&profile, &signals, &catalog.recent);
    Ok(Ranking {
        resolved,
        ranked,
        recent,
    })
}

/// Leave-one-out evaluation of the scoring on the user's own ratings (see
/// [`evaluate::leave_one_out`]), using the shared catalog's rarity table like real
/// rankings do.
pub async fn evaluate_ratings(
    fetcher: &Fetcher,
    movies: &[RatedMovie],
    options: &Options,
) -> Result<(Resolved, Evaluation)> {
    let catalog = Catalog::load(fetcher, options.catalog).await?;
    let resolved = resolve(fetcher, movies).await?;
    let evaluation = evaluate::leave_one_out(
        &resolved.rated,
        &catalog.idf(&options.weights),
        options.weights,
    );
    Ok((resolved, evaluation))
}

/// The result of [`evaluate_movielens`].
#[derive(Debug, Clone)]
pub struct MovieLensEvaluation {
    /// Distinct movies across all the users' ratings.
    pub movies_requested: usize,
    /// How many of them TMDB has (the rest answered 404 and are skipped).
    pub movies_found: usize,
    /// People whose filmographies were needed for quality estimates, if made.
    pub people: Option<usize>,
    pub report: UsersReport,
    /// With quality estimates: movie-match plus a damped track-record nudge, tuned
    /// on half the users and checked on the other half.
    pub nudge: Option<evaluate::NudgeReport>,
}

/// Leave-one-out evaluation for every user in `users` (see
/// [`evaluate::leave_one_out_each`]). Fetches TMDB details for all their movies,
/// trimmed to the fields scoring uses, and builds the rarity table from those same
/// movies. Users with fewer than two movies TMDB has are skipped.
///
/// With `quality_cast` set, also estimates each movie's audience score from its
/// directors', writers' and top `quality_cast` actors' earlier films (fetching their
/// filmographies) for the "est. quality" methods; the trimmed details then keep that
/// many actors even past `weights.cast_limit`.
pub async fn evaluate_movielens(
    fetcher: &Fetcher,
    users: &UserRatings,
    weights: FieldWeights,
    quality_cast: Option<usize>,
) -> Result<MovieLensEvaluation> {
    let data = movielens_data(fetcher, users, weights, quality_cast).await?;
    let per_user: Vec<Vec<RatedDetails>> = users
        .values()
        .map(|ratings| {
            ratings
                .iter()
                .filter_map(|(id, rating)| {
                    Some(RatedDetails {
                        movie: data.details.get(id)?.clone(),
                        rating: *rating,
                    })
                })
                .collect::<Vec<_>>()
        })
        .filter(|rated| rated.len() >= 2)
        .collect();
    let evaluations =
        evaluate::leave_one_out_each(&per_user, &data.idf, weights, data.estimates.as_ref());
    Ok(MovieLensEvaluation {
        movies_requested: data.requested,
        movies_found: data.details.len(),
        people: data.people,
        report: evaluate::across_users(&evaluations),
        nudge: data
            .estimates
            .as_ref()
            .and_then(|e| evaluate::tune_nudge(&evaluations, e.typical)),
    })
}

/// The result of [`benchmark_movielens`].
#[derive(Debug, Clone)]
pub struct MovieLensBenchmark {
    /// Distinct movies across all the users' ratings.
    pub movies_requested: usize,
    /// How many of them TMDB has.
    pub movies_found: usize,
    /// People whose filmographies were needed for track records, if used.
    pub people: Option<usize>,
    pub report: BenchmarkReport,
}

/// Movie Match against standard recommenders on every user in `users` (all their
/// ratings, newest first), hiding each user's newest [`benchmark::TEST_SHARE`] (see
/// [`benchmark::run`]). Fetches TMDB details for every rated movie and builds the
/// rarity table from them; with `quality_cast`, Movie Match also gets its track-record
/// nudge, as in [`evaluate_movielens`].
pub async fn benchmark_movielens(
    fetcher: &Fetcher,
    users: &UserRatings,
    weights: FieldWeights,
    quality_cast: Option<usize>,
) -> Result<MovieLensBenchmark> {
    let data = movielens_data(fetcher, users, weights, quality_cast).await?;
    let split = benchmark::split(users, benchmark::TEST_SHARE, benchmark::MIN_TRAIN);
    let report = benchmark::run(
        &split,
        &data.details,
        &data.idf,
        weights,
        data.estimates.as_ref(),
        BENCHMARK_SEED,
    );
    Ok(MovieLensBenchmark {
        movies_requested: data.requested,
        movies_found: data.details.len(),
        people: data.people,
        report,
    })
}

/// Seeds SVD's starting factors, so benchmark runs repeat exactly.
const BENCHMARK_SEED: u64 = 42;

/// What the MovieLens evaluations need from TMDB.
struct MovieLensData {
    /// Distinct movies across all the users' ratings.
    requested: usize,
    /// Details for those TMDB has, trimmed to the fields scoring uses.
    details: BTreeMap<u64, MovieDetails>,
    /// Rarity table from those same movies.
    idf: Idf,
    /// Track-record estimates for every movie, with `quality_cast`.
    estimates: Option<TrackRecords>,
    /// People whose filmographies the estimates needed.
    people: Option<usize>,
}

/// Fetches TMDB details for every movie in `users`, trimmed to the fields scoring
/// uses, and builds the rarity table from those same movies. With `quality_cast` set,
/// also estimates each movie's audience score from its directors', writers' and top
/// `quality_cast` actors' earlier films (fetching their filmographies); the trimmed
/// details then keep that many actors even past `weights.cast_limit`.
async fn movielens_data(
    fetcher: &Fetcher,
    users: &UserRatings,
    weights: FieldWeights,
    quality_cast: Option<usize>,
) -> Result<MovieLensData> {
    let ids: Vec<u64> = users
        .values()
        .flatten()
        .map(|(id, _)| *id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut details = BTreeMap::new();
    for (id, found) in ids.iter().zip(fetcher.details_if_found(&ids).await?) {
        if let Some(mut movie) = found {
            features::trim_keeping_cast(&mut movie, &weights, quality_cast.unwrap_or(0));
            details.insert(*id, movie);
        }
    }
    let documents: Vec<_> = details
        .values()
        .map(|movie| features::extract(movie, &weights))
        .collect();
    let idf = Idf::from_documents(&documents);

    let (estimates, people) = match quality_cast {
        None => (None, None),
        Some(cast_limit) => {
            let people: Vec<u64> = details
                .values()
                .flat_map(|movie| quality::credits(movie, &weights, cast_limit))
                .map(|credit| credit.person)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let filmographies: HashMap<u64, Vec<CreditedFilm>> = people
                .iter()
                .zip(fetcher.filmographies(&people).await?)
                .filter_map(|(person, films)| Some((*person, films?)))
                .collect();
            let estimates = quality::typical_score(&filmographies).map(|typical| TrackRecords {
                typical,
                people: HashMap::new(),
                estimates: details
                    .values()
                    .map(|movie| {
                        let credits = quality::credits(movie, &weights, cast_limit);
                        let found = quality::estimate(movie, &credits, &filmographies, typical);
                        (movie.id, found.unwrap_or(typical))
                    })
                    .collect(),
            });
            (estimates, Some(people.len()))
        }
    };
    Ok(MovieLensData {
        requested: ids.len(),
        details,
        idf,
        estimates,
        people,
    })
}

/// The result of [`evaluate_track_record`].
#[derive(Debug, Clone)]
pub struct TrackRecordEvaluation {
    /// Distinct people whose filmographies were needed.
    pub people: usize,
    pub report: quality::TrackRecordReport,
    /// The estimator's settings tuned on half the films, checked on the other half.
    pub tuning: quality::TuningReport,
}

/// How well cast and crew track records predict the audience score of `films`
/// (see [`quality::evaluate_track_record`]), plus split-half tuning of the
/// estimator's settings (see [`quality::tune`]). Fetches the films' details and
/// every credited person's filmography (cached, trimmed).
pub async fn evaluate_track_record(
    fetcher: &Fetcher,
    films: &[u64],
    weights: FieldWeights,
    cast_limit: usize,
) -> Result<TrackRecordEvaluation> {
    let targets: Vec<MovieDetails> = fetcher
        .details_if_found(films)
        .await?
        .into_iter()
        .flatten()
        .map(|mut movie| {
            features::trim_keeping_cast(&mut movie, &weights, cast_limit);
            movie
        })
        .collect();
    let people: Vec<u64> = targets
        .iter()
        .flat_map(|movie| quality::credits(movie, &weights, cast_limit))
        .map(|credit| credit.person)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let filmographies = people
        .iter()
        .zip(fetcher.filmographies(&people).await?)
        .filter_map(|(person, films)| Some((*person, films?)))
        .collect();
    Ok(TrackRecordEvaluation {
        people: people.len(),
        report: quality::evaluate_track_record(&targets, &filmographies, &weights, cast_limit),
        tuning: quality::tune(&targets, &filmographies, &weights, cast_limit),
    })
}

/// One movie's score for this user, spelled out.
#[derive(Debug, Clone)]
pub struct MovieExplanation {
    pub movie: MovieDetails,
    pub explanation: Explanation,
    /// The user's own rating, when they've rated this movie. The explanation then
    /// leaves that rating out, predicting from their other ratings as if unseen.
    pub your_rating: Option<f32>,
}

/// Explains one movie's score for this user. `title`, trimmed of surrounding
/// whitespace, is matched against the upcoming and recent releases first (ignoring
/// case), then looked up with a TMDB search, so any movie works. `None` when nothing
/// matches.
pub async fn explain_movie(
    fetcher: &Fetcher,
    movies: &[RatedMovie],
    options: &Options,
    title: &str,
) -> Result<Option<MovieExplanation>> {
    let title = title.trim();
    let catalog = Catalog::load(fetcher, options.catalog).await?;
    let movie = match catalog
        .upcoming
        .iter()
        .chain(&catalog.recent)
        .find(|m| m.title.eq_ignore_ascii_case(title))
    {
        Some(movie) => movie.clone(),
        None => {
            let found = fetcher
                .client()
                .search_movie(title, None)
                .await
                .with_context(|| format!("searching TMDB for \"{title}\""))?;
            let Some(found) = found else {
                return Ok(None);
            };
            let mut details = fetcher.details(&[found.id]).await?;
            match details.pop() {
                Some(movie) => movie,
                None => return Ok(None),
            }
        }
    };
    explain_details(fetcher, movies, options, &catalog, movie)
        .await
        .map(Some)
}

/// TMDB's matches for `query`, trimmed of surrounding whitespace, best first: the
/// movies to pick from before [`explain_movie_id`].
pub async fn search_movies(fetcher: &Fetcher, query: &str) -> Result<Vec<Movie>> {
    let query = query.trim();
    fetcher
        .client()
        .search_movies(query, None)
        .await
        .with_context(|| format!("searching TMDB for \"{query}\""))
}

/// Explains the score of TMDB movie `id` for this user, like [`explain_movie`]. `None`
/// when TMDB has no such movie.
pub async fn explain_movie_id(
    fetcher: &Fetcher,
    movies: &[RatedMovie],
    options: &Options,
    id: u64,
) -> Result<Option<MovieExplanation>> {
    let catalog = Catalog::load(fetcher, options.catalog).await?;
    let Some(movie) = fetcher.details_if_found(&[id]).await?.pop().flatten() else {
        return Ok(None);
    };
    explain_details(fetcher, movies, options, &catalog, movie)
        .await
        .map(Some)
}

/// `movie`'s explanation against `catalog`, which supplies the IDF and the typical
/// track record.
async fn explain_details(
    fetcher: &Fetcher,
    movies: &[RatedMovie],
    options: &Options,
    catalog: &Catalog,
    movie: MovieDetails,
) -> Result<MovieExplanation> {
    let mut resolved = resolve(fetcher, movies).await?;
    // A movie the user rated is predicted from their other ratings, as if unseen:
    // otherwise it would be its own closest match and part of the fitted shift.
    let your_rating = resolved
        .rated
        .iter()
        .find(|r| r.movie.id == movie.id)
        .map(|r| r.rating);
    resolved.rated.retain(|r| r.movie.id != movie.id);
    let profile = TasteProfile::new(
        &resolved.rated,
        catalog.idf(&options.weights),
        options.weights,
    );
    // The typical score comes from the whole catalog's track records, as when
    // ranking, so a catalog movie is explained with the score it's listed with.
    let candidates: Vec<&MovieDetails> = catalog
        .upcoming
        .iter()
        .chain(&catalog.recent)
        .chain([&movie])
        .collect();
    let signals = signals(fetcher, &resolved, &candidates, options.weights, &|_| {}).await?;
    let explanation = scoring::explain(&profile, &signals, &movie);
    Ok(MovieExplanation {
        movie,
        explanation,
        your_rating,
    })
}

/// What scoring uses beyond similarity: the user's [`AudienceShift`] and the
/// `candidates`' track records (see [`track_records`]).
async fn signals(
    fetcher: &Fetcher,
    resolved: &Resolved,
    candidates: &[&MovieDetails],
    weights: FieldWeights,
    report: &(dyn Fn(Stage) + Send + Sync),
) -> Result<Signals> {
    Ok(Signals {
        shift: AudienceShift::fit(&resolved.rated, weights),
        track_records: track_records(fetcher, candidates, &weights, report).await?,
    })
}

/// Each of `movies`' estimated audience score from its directors', writers' and
/// top [`TRACK_RECORD_CAST`] actors' earlier films (see [`quality::estimate`]),
/// fetching their filmographies (cached for a month). A person whose films fail to
/// load is left out (see [`Fetcher::filmographies_or_skip`]). The typical score
/// comes from those same filmographies. `None` when none of their films has enough
/// votes to set it.
async fn track_records(
    fetcher: &Fetcher,
    movies: &[&MovieDetails],
    weights: &FieldWeights,
    report: &(dyn Fn(Stage) + Send + Sync),
) -> Result<Option<TrackRecords>> {
    let credits: Vec<Vec<quality::Credit>> = movies
        .iter()
        .map(|movie| quality::credits(movie, weights, TRACK_RECORD_CAST))
        .collect();
    let people: Vec<u64> = credits
        .iter()
        .flatten()
        .map(|credit| credit.person)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    report(Stage::TrackRecords {
        people: people.len(),
    });
    // Track records only nudge a score, so a person whose films don't load is left
    // out (with a warning) rather than failing the ranking.
    let filmographies: HashMap<u64, Vec<CreditedFilm>> = people
        .iter()
        .zip(fetcher.filmographies_or_skip(&people).await)
        .filter_map(|(person, films)| Some((*person, films?)))
        .collect();
    let Some(typical) = quality::typical_score(&filmographies) else {
        return Ok(None);
    };
    let mut records = TrackRecords {
        typical,
        ..TrackRecords::default()
    };
    for (movie, credits) in movies.iter().zip(&credits) {
        if let Some(estimate) = quality::estimate(movie, credits, &filmographies, typical) {
            records.estimates.insert(movie.id, estimate);
            let people = quality::records(movie, credits, &filmographies, typical);
            records.people.insert(movie.id, people);
        }
    }
    Ok(Some(records))
}

/// Matches each rated movie to TMDB (by id, or else by title search) and fetches
/// its details. A movie listed twice keeps its first rating.
pub async fn resolve(fetcher: &Fetcher, movies: &[RatedMovie]) -> Result<Resolved> {
    // Owned items (not references) keep these futures `Send`, so callers can run
    // them on a multi-threaded runtime.
    let lookups: Vec<_> = stream::iter(movies.to_vec())
        .map(|movie| async move {
            let found = lookup_id(fetcher, &movie).await;
            (movie, found)
        })
        .buffered(CONCURRENT_REQUESTS)
        .collect()
        .await;

    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    let mut ratings = Vec::new();
    let mut resolved = Resolved::default();
    for (wanted, found) in lookups {
        let found = found.with_context(|| format!("looking up \"{}\" on TMDB", wanted.label()))?;
        let Some((id, search_match)) = found else {
            resolved.unmatched.push(wanted.label());
            resolved.ids.push(None);
            continue;
        };
        resolved.ids.push(Some(id));
        if !seen.insert(id) {
            continue;
        }
        ids.push(id);
        ratings.push(wanted.rating());
        if let Some(search_match) = search_match {
            resolved.searched.push((wanted.label(), search_match));
        }
    }

    // An id TMDB answers 404 for (since removed, mistyped, or not a movie) counts as
    // unmatched rather than failing, so one bad saved id can't break every run.
    let details = fetcher.details_if_found(&ids).await?;
    let mut missing = HashSet::new();
    for ((&id, found), rating) in ids.iter().zip(details).zip(ratings) {
        match found {
            Some(movie) => resolved.rated.push(RatedDetails { movie, rating }),
            None => {
                missing.insert(id);
            }
        }
    }
    for (movie, id) in movies.iter().zip(&mut resolved.ids) {
        if id.is_some_and(|id| missing.contains(&id)) {
            *id = None;
            resolved.unmatched.push(movie.label());
        }
    }
    Ok(resolved)
}

/// The TMDB id for `movie`, plus a description of the match if a title search
/// was needed to find it.
async fn lookup_id(fetcher: &Fetcher, movie: &RatedMovie) -> Result<Option<(u64, Option<String>)>> {
    if let Some(id) = movie.tmdb_id {
        return Ok(Some((id, None)));
    }
    let Some(title) = movie.title.as_deref() else {
        return Ok(None);
    };
    let found = fetcher.client().search_movie(title, movie.year).await?;
    Ok(found.map(|m| {
        let year = m.release_date.get(..4).unwrap_or("unknown year");
        (m.id, Some(format!("{} ({year})", m.title)))
    }))
}
