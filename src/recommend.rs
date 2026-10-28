//! The per-user steps: resolve a user's ratings to TMDB movies, then rank the
//! shared catalog against them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};

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
use crate::tmdb::{CreditedFilm, MovieDetails};

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

/// Ranks the catalog's upcoming and recent releases for one user (see
/// [`scoring::rank_with`]), with their difference from the audience and the
/// candidates' cast and crew track records.
pub async fn rank_upcoming(
    fetcher: &Fetcher,
    movies: &[RatedMovie],
    options: &Options,
) -> Result<Ranking> {
    let catalog = Catalog::load(fetcher, options.catalog).await?;
    let resolved = resolve(fetcher, movies).await?;
    let profile = TasteProfile::new(
        &resolved.rated,
        catalog.idf(&options.weights),
        options.weights,
    );
    let candidates: Vec<&MovieDetails> = catalog.upcoming.iter().chain(&catalog.recent).collect();
    let signals = signals(fetcher, &resolved, &candidates, options.weights).await?;
    let ranked = scoring::rank_with(&profile, &signals, &catalog.upcoming);
    let recent = scoring::rank_with(&profile, &signals, &catalog.recent);
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
    let per_user: Vec<Vec<RatedDetails>> = users
        .values()
        .map(|ratings| {
            ratings
                .iter()
                .filter_map(|(id, rating)| {
                    Some(RatedDetails {
                        movie: details.get(id)?.clone(),
                        rating: *rating,
                    })
                })
                .collect::<Vec<_>>()
        })
        .filter(|rated| rated.len() >= 2)
        .collect();

    let mut people_needed = None;
    let mut typical_quality = None;
    let estimates: Option<TrackRecords> = match quality_cast {
        None => None,
        Some(cast_limit) => {
            let people: Vec<u64> = details
                .values()
                .flat_map(|movie| quality::credits(movie, &weights, cast_limit))
                .map(|credit| credit.person)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            people_needed = Some(people.len());
            let filmographies: HashMap<u64, Vec<CreditedFilm>> = people
                .iter()
                .zip(fetcher.filmographies(&people).await?)
                .filter_map(|(person, films)| Some((*person, films?)))
                .collect();
            typical_quality = quality::typical_score(&filmographies);
            typical_quality.map(|typical| TrackRecords {
                typical,
                estimates: details
                    .values()
                    .map(|movie| {
                        let credits = quality::credits(movie, &weights, cast_limit);
                        let found = quality::estimate(movie, &credits, &filmographies, typical);
                        (movie.id, found.unwrap_or(typical))
                    })
                    .collect(),
            })
        }
    };

    let evaluations = evaluate::leave_one_out_each(&per_user, &idf, weights, estimates.as_ref());
    Ok(MovieLensEvaluation {
        movies_requested: ids.len(),
        movies_found: details.len(),
        people: people_needed,
        report: evaluate::across_users(&evaluations),
        nudge: typical_quality.and_then(|typical| evaluate::tune_nudge(&evaluations, typical)),
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

/// Explains one movie's score for this user. `title`, trimmed of surrounding
/// whitespace, is matched against the upcoming and recent releases first (ignoring
/// case), then looked up with a TMDB search, so any movie works. `None` when nothing
/// matches.
pub async fn explain_movie(
    fetcher: &Fetcher,
    movies: &[RatedMovie],
    options: &Options,
    title: &str,
) -> Result<Option<(MovieDetails, Explanation)>> {
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
    let resolved = resolve(fetcher, movies).await?;
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
    let signals = signals(fetcher, &resolved, &candidates, options.weights).await?;
    let explanation = scoring::explain_with(&profile, &signals, &movie);
    Ok(Some((movie, explanation)))
}

/// What scoring uses beyond similarity: the user's [`AudienceShift`] and the
/// `candidates`' track records (see [`track_records`]).
async fn signals(
    fetcher: &Fetcher,
    resolved: &Resolved,
    candidates: &[&MovieDetails],
    weights: FieldWeights,
) -> Result<Signals> {
    Ok(Signals {
        shift: AudienceShift::fit(&resolved.rated, weights),
        track_records: track_records(fetcher, candidates, &weights).await?,
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
    let estimates = movies
        .iter()
        .zip(&credits)
        .filter_map(|(movie, credits)| {
            let estimate = quality::estimate(movie, credits, &filmographies, typical)?;
            Some((movie.id, estimate))
        })
        .collect();
    Ok(Some(TrackRecords { typical, estimates }))
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
