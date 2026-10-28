use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use movie_match::cache::DiskCache;
use movie_match::catalog::CatalogOptions;
use movie_match::config::Config;
use movie_match::evaluate;
use movie_match::fetch::Fetcher;
use movie_match::movielens;
use movie_match::profile::{self, RatedMovie, DISLIKED_UP_TO, LIKED_FROM};
use movie_match::recommend::{self, Options};
use movie_match::tmdb::{Region, TmdbClient};

/// Rate upcoming movies by how likely you are to enjoy them.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// TOML file listing movies you've rated (see data/liked_movies.example.toml).
    #[arg(short, long, default_value = "data/liked_movies.toml")]
    liked: PathBuf,

    /// Pages of upcoming releases (today or later) to rate (20 movies per page).
    #[arg(long, default_value_t = 2)]
    pages: u32,

    /// Rank recent releases (before today, within --recent-months) instead of
    /// upcoming ones.
    #[arg(long)]
    recent: bool,

    /// Only rank upcoming movies with a cinema release listed for this country (a
    /// two-letter code such as US or GB). Without it, every upcoming movie on TMDB.
    #[arg(long, value_parser = parse_region)]
    region: Option<Region>,

    /// Pages of recent releases to rate per 3 months of --recent-months (20 movies
    /// per page).
    #[arg(long, default_value_t = 1)]
    recent_pages: u32,

    /// How far back "recent" reaches, in months (1 to 120).
    #[arg(long, default_value_t = 12, value_parser = clap::value_parser!(u32).range(1..=120))]
    recent_months: u32,

    /// Recent releases need at least this many TMDB audience votes.
    #[arg(long, default_value_t = 50)]
    recent_min_votes: u32,

    /// Pages of popular movies used to judge how rare each feature is.
    #[arg(long, default_value_t = 5)]
    reference_pages: u32,

    /// How many results to show.
    #[arg(short = 'n', long, default_value_t = 20)]
    top: usize,

    /// Where downloaded TMDB data is cached between runs.
    #[arg(long, default_value = "data/cache")]
    cache_dir: PathBuf,

    /// Ignore cached data and download everything again.
    #[arg(long)]
    refresh: bool,

    /// Don't read or write the cache at all.
    #[arg(long, conflicts_with = "refresh")]
    no_cache: bool,

    /// Instead of ranking, show how one movie's score was predicted: its nearest
    /// rated movies, their similarity and your rating. Any title TMDB can find works.
    #[arg(long, value_name = "TITLE")]
    explain: Option<String>,

    /// Instead of ranking, measure how well scoring predicts your own ratings: hide
    /// each rated movie in turn, predict it from the rest, and compare with simple
    /// baselines.
    #[arg(long, conflicts_with = "explain")]
    evaluate: bool,

    /// Developer tool: run that evaluation for every user in a MovieLens download
    /// (the folder holding ratings.csv and links.csv) and compare the methods across
    /// users. Use a separate --cache-dir: it fetches thousands of movies.
    #[arg(long, value_name = "DIR", conflicts_with_all = ["explain", "evaluate"])]
    evaluate_movielens: Option<PathBuf>,

    /// With --evaluate-movielens: how many of each user's most recent ratings to use.
    #[arg(long, default_value_t = 150)]
    movielens_ratings: usize,

    /// With --evaluate-movielens: also estimate every film's quality from its cast
    /// and crew's earlier films and add the "est. quality" methods, which treat each
    /// hidden movie as unreleased. Fetches a filmography per person (thousands).
    #[arg(long)]
    movielens_quality: bool,

    /// Developer tool: how well cast and crew track records (their earlier films'
    /// audience scores) predict a film's TMDB audience score, on films from a
    /// MovieLens download's links.csv. Use a separate --cache-dir.
    #[arg(long, value_name = "DIR", conflicts_with_all = ["explain", "evaluate", "evaluate_movielens"])]
    evaluate_track_record: Option<PathBuf>,

    /// With --evaluate-track-record: how many films to test, spread evenly over the list.
    #[arg(long, default_value_t = 1000)]
    track_record_films: usize,

    /// With --evaluate-track-record: how many top-billed actors' records to use.
    #[arg(long, default_value_t = 3)]
    track_record_cast: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::from_env()?;

    let cache = (!cli.no_cache).then(|| {
        let cache = DiskCache::new(&cli.cache_dir);
        if cli.refresh {
            cache.refreshing()
        } else {
            cache
        }
    });
    let fetcher = Fetcher::new(TmdbClient::new(config.tmdb_api_key), cache);
    let options = Options {
        catalog: CatalogOptions {
            upcoming_pages: cli.pages,
            upcoming_region: cli.region,
            recent_pages: cli.recent_pages,
            recent_months: cli.recent_months,
            recent_min_votes: cli.recent_min_votes,
            reference_pages: cli.reference_pages,
        },
        ..Options::default()
    };
    if let Some(dir) = &cli.evaluate_movielens {
        let quality_cast = cli.movielens_quality.then_some(cli.track_record_cast);
        return evaluate_movielens(&fetcher, dir, cli.movielens_ratings, quality_cast, &options)
            .await;
    }
    if let Some(dir) = &cli.evaluate_track_record {
        return evaluate_track_record(
            &fetcher,
            dir,
            cli.track_record_films,
            cli.track_record_cast,
            &options,
        )
        .await;
    }
    let movies = profile::load_rated_movies(&cli.liked)?;
    if let Some(title) = &cli.explain {
        return explain(&fetcher, &movies, &options, title).await;
    }
    if cli.evaluate {
        return evaluate(&fetcher, &movies, &options).await;
    }
    let ranking = recommend::rank_upcoming(&fetcher, &movies, &options).await?;
    let resolved = &ranking.resolved;

    let liked = resolved.rated.iter().filter(|r| r.rating >= LIKED_FROM);
    let disliked = resolved.rated.iter().filter(|r| r.rating <= DISLIKED_UP_TO);
    println!(
        "Using {} rated movies: {} liked, {} disliked.",
        resolved.rated.len(),
        liked.count(),
        disliked.count()
    );
    if !resolved.searched.is_empty() {
        println!("Found by title search (add a tmdb_id if any are wrong):");
        for (wanted, found) in &resolved.searched {
            println!("  \"{wanted}\" -> {found}");
        }
    }
    for title in &resolved.unmatched {
        eprintln!("warning: no TMDB match for \"{title}\"");
    }

    let (heading, list) = if cli.recent {
        (
            format!(
                "Recent releases (the last {} months), best match first:",
                cli.recent_months
            ),
            &ranking.recent,
        )
    } else {
        (
            "Upcoming releases (today or later), best match first:".to_string(),
            &ranking.ranked,
        )
    };
    println!("\n{heading}");
    for ranked in list.iter().take(cli.top) {
        let audience = match (ranked.movie.audience_rating(), ranked.movie.vote_count) {
            (Some(score), Some(votes)) => format!("  audience {score:.1} ({votes} votes)"),
            _ => String::new(),
        };
        println!(
            "  {:>3}  {} [{}]{audience}",
            ranked.score, ranked.movie.title, ranked.movie.release_date
        );
        if let Some((title, rating)) = &ranked.closest_liked {
            let because = if ranked.reasons.is_empty() {
                String::new()
            } else {
                format!(": {}", ranked.reasons.join(", "))
            };
            println!("       like {title} ({}/10){because}", rating_text(*rating));
        }
        if let Some((title, rating)) = &ranked.closest_disliked {
            println!(
                "       but resembles {title}, which you rated {}/10",
                rating_text(*rating)
            );
        }
    }

    Ok(())
}

/// `9` for whole ratings, `7.5` otherwise.
fn rating_text(rating: f32) -> String {
    if rating.fract() == 0.0 {
        format!("{rating:.0}")
    } else {
        format!("{rating:.1}")
    }
}

async fn explain(
    fetcher: &Fetcher,
    movies: &[RatedMovie],
    options: &Options,
    title: &str,
) -> Result<()> {
    let Some((movie, explanation)) =
        recommend::explain_movie(fetcher, movies, options, title).await?
    else {
        eprintln!("No movie found for \"{title}\"");
        return Ok(());
    };
    println!(
        "{} [{}]: score {} (predicted {:.2}/10)",
        movie.title, movie.release_date, explanation.score, explanation.predicted_rating
    );
    if !explanation.neighbours.is_empty() {
        println!(
            "Weighted average of these movies: {:.2}; your overall average {:.2} makes up {:.0}% of the score ({} neighbours).",
            explanation.neighbour_rating,
            explanation.mean_rating,
            explanation.overall_share * 100.0,
            explanation.neighbours.len()
        );
        println!("Nearest rated movies (weight = (similarity / closest similarity)²):");
    }
    if explanation.neighbours.is_empty() {
        println!(
            "  none share any features, so the score is your overall average ({:.2})",
            explanation.mean_rating
        );
    }
    for (i, n) in explanation.neighbours.iter().enumerate() {
        println!(
            "  {:>2}. {}  (you rated {}/10)  similarity {:.3}, weight {:.2}",
            i + 1,
            n.title,
            rating_text(n.rating),
            n.similarity,
            n.weight
        );
        if !n.shared.is_empty() {
            println!("      shares: {}", n.shared.join(", "));
        }
    }
    Ok(())
}

async fn evaluate(fetcher: &Fetcher, movies: &[RatedMovie], options: &Options) -> Result<()> {
    let (resolved, evaluation) = recommend::evaluate_ratings(fetcher, movies, options).await?;
    for title in &resolved.unmatched {
        eprintln!("warning: no TMDB match for \"{title}\"");
    }
    let total = evaluation.movies.len();
    if total < 2 {
        eprintln!("Need at least two rated movies found on TMDB to evaluate.");
        return Ok(());
    }
    println!(
        "Leave-one-out on {total} rated movies: each predicted from the other {}.\n",
        total - 1
    );
    println!(
        "  {:<26} {:>6} {:>6} {:>9} {:>7}",
        "method", "MAE", "RMSE", "Spearman", "movies"
    );
    for result in &evaluation.results {
        let spearman = result
            .spearman
            .map_or_else(|| "-".to_string(), |s| format!("{s:.2}"));
        println!(
            "  {:<26} {:>6.2} {:>6.2} {:>9} {:>7}",
            result.name, result.mae, result.rmse, spearman, result.count
        );
    }
    println!(
        "\nMAE/RMSE: average miss in rating points (lower is better; RMSE punishes big misses).\n\
         Spearman: how well the order matches yours, -1 to 1 (higher is better).\n\
         diff = your rating minus the TMDB audience score. shift = a learned diff per genre,\n\
         director, writer, actor and keyword; light/medium/strong is how hard thin evidence\n\
         is pulled toward no shift."
    );

    let mut misses: Vec<_> = evaluation
        .movies
        .iter()
        .filter_map(|m| Some((m, m.predictions[0]?)))
        .collect();
    misses.sort_by(|a, b| {
        (b.1 - b.0.rating)
            .abs()
            .total_cmp(&(a.1 - a.0.rating).abs())
    });
    println!("\nBiggest movie-match misses:");
    for (movie, predicted) in misses.iter().take(5) {
        println!(
            "  {}: you rated {}, predicted {:.1}",
            movie.title,
            rating_text(movie.rating),
            predicted
        );
    }
    Ok(())
}

async fn evaluate_movielens(
    fetcher: &Fetcher,
    dir: &std::path::Path,
    per_user: usize,
    quality_cast: Option<usize>,
    options: &Options,
) -> Result<()> {
    let read = |name: &str| {
        let path = dir.join(name);
        std::fs::read_to_string(&path).with_context(|| format!("could not read {}", path.display()))
    };
    let links = movielens::parse_links(&read("links.csv")?)?;
    let users = movielens::recent_ratings(&read("ratings.csv")?, &links, per_user)?;
    let movies: std::collections::BTreeSet<u64> =
        users.values().flatten().map(|(id, _)| *id).collect();
    eprintln!(
        "Fetching TMDB details for {} movies rated by {} users (cached after the first run), then evaluating...",
        movies.len(),
        users.len()
    );
    let result =
        recommend::evaluate_movielens(fetcher, &users, options.weights, quality_cast).await?;
    let report = &result.report;
    let predictions = report.pooled.first().map_or(0, |r| r.count);
    println!(
        "MovieLens: {} users, {predictions} predictions (each user's {per_user} most recent ratings at most).",
        report.users
    );
    println!(
        "TMDB had {} of the {} movies.",
        result.movies_found, result.movies_requested
    );
    if let Some(people) = result.people {
        println!(
            "Quality estimates from {people} people's earlier films. \"est. quality\" rows treat each \
             hidden movie as unreleased."
        );
    }
    println!();

    // Methods that made no predictions (est. quality without estimates) are left out.
    let shown: Vec<usize> = (0..report.pooled.len())
        .filter(|&m| report.pooled[m].count > 0)
        .collect();
    println!(
        "  {:<30} {:>6} {:>6} {:>14} {:>6}",
        "method", "MAE", "RMSE", "mean Spearman", "wins"
    );
    for &m in &shown {
        let pooled = &report.pooled[m];
        let spearman =
            report.mean_spearman[m].map_or_else(|| "-".to_string(), |s| format!("{s:.2}"));
        println!(
            "  {:<30} {:>6.2} {:>6.2} {:>14} {:>5.0}%",
            pooled.name,
            pooled.mae,
            pooled.rmse,
            spearman,
            report.wins[m] * 100.0
        );
    }
    println!(
        "\nMAE/RMSE pool every prediction. mean Spearman averages each user's own order match.\n\
         wins: share of users for whom the method had the lowest MAE."
    );

    println!(
        "\nMAE by how closely users follow the TMDB audience (Spearman of their ratings vs audience scores):"
    );
    print!("  {:<30}", "method");
    for (g, group) in report.taste_groups.iter().enumerate() {
        print!(
            " {:>17}",
            format!(
                "Q{} {:.2}..{:.2}",
                g + 1,
                group.agreement.0,
                group.agreement.1
            )
        );
    }
    println!();
    for &m in &shown {
        let name = evaluate::METHODS[m];
        print!("  {name:<30}");
        for group in &report.taste_groups {
            print!(" {:>17.2}", group.mae[m]);
        }
        println!();
    }
    let sizes: Vec<String> = report
        .taste_groups
        .iter()
        .map(|g| g.users.to_string())
        .collect();
    println!(
        "Q1 follows the crowd most, the last group least. Users per group: {}.",
        sizes.join(", ")
    );

    if let Some(nudge) = &result.nudge {
        println!(
            "\nmovie-match + strength x (estimated quality - typical): strength tuned on {} users, \
             checked on the other {}.",
            nudge.tune_users, nudge.check_users
        );
        println!(
            "  {:<10} {:>6} {:>6} {:>14}",
            "strength", "MAE", "RMSE", "mean Spearman"
        );
        for (strength, scores) in &nudge.check {
            let spearman = scores
                .mean_spearman
                .map_or_else(|| "-".to_string(), |s| format!("{s:.3}"));
            let marks = match (*strength == 0.0, *strength == nudge.best) {
                (true, true) => "  <- current scoring, and tuned",
                (true, false) => "  <- current scoring",
                (false, true) => "  <- tuned",
                (false, false) => "",
            };
            println!(
                "  {:<10} {:>6.3} {:>6.3} {:>14}{marks}",
                strength, scores.mae, scores.rmse, spearman
            );
        }
        println!("All rows are on the check users, which tuning never saw.");
    }
    Ok(())
}

async fn evaluate_track_record(
    fetcher: &Fetcher,
    dir: &std::path::Path,
    films: usize,
    cast_limit: usize,
    options: &Options,
) -> Result<()> {
    let path = dir.join("links.csv");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let all: Vec<u64> = movielens::parse_links(&text)?
        .into_values()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    // An even spread over the TMDB ids, so the sample is the same every run.
    let step = (all.len() / films.max(1)).max(1);
    let sample: Vec<u64> = all.iter().step_by(step).take(films).copied().collect();
    eprintln!(
        "Fetching {} films and their people's filmographies (cached after the first run)...",
        sample.len()
    );
    let result =
        recommend::evaluate_track_record(fetcher, &sample, options.weights, cast_limit).await?;
    let report = &result.report;
    println!(
        "Track records on {} films with at least {} votes ({} people's filmographies).",
        report.films,
        movie_match::quality::MIN_TRUTH_VOTES,
        result.people
    );
    println!(
        "Each estimate uses only films its people made before it. Typical score: {:.2}.\n",
        report.typical
    );
    println!(
        "  {:<22} {:>9} {:>6} {:>6} {:>9}",
        "estimate from", "coverage", "MAE", "RMSE", "Spearman"
    );
    for r in &report.results {
        let spearman = r
            .spearman
            .map_or_else(|| "-".to_string(), |s| format!("{s:.2}"));
        println!(
            "  {:<22} {:>8.0}% {:>6.2} {:>6.2} {:>9}",
            r.name,
            r.coverage * 100.0,
            r.mae,
            r.rmse,
            spearman
        );
    }
    println!(
        "\ncoverage: films with any earlier film by those people (the rest get the typical score).\n\
         MAE/RMSE: miss against the real TMDB audience score. Spearman: how well it orders films."
    );

    let tuning = &result.tuning;
    let best = tuning.best;
    println!(
        "\nTuning \"everyone's records\": settings chosen on {} films, checked on the other {}.",
        tuning.tune_films, tuning.check_films
    );
    println!(
        "  best: prior votes {}, person shrink {}, estimate shrink {}  \
         (defaults: {}, {}, {})",
        best.prior_votes,
        best.person_shrink,
        best.estimate_shrink,
        movie_match::quality::PRIOR_VOTES,
        movie_match::quality::PERSON_SHRINK,
        movie_match::quality::ESTIMATE_SHRINK
    );
    let row = |name: &str, s: &movie_match::quality::Scores| {
        let spearman = s
            .spearman
            .map_or_else(|| "-".to_string(), |v| format!("{v:.2}"));
        println!(
            "  {name:<34} {:>6.3} {:>6.3} {:>9}",
            s.mae, s.rmse, spearman
        );
    };
    println!(
        "\n  {:<34} {:>6} {:>6} {:>9}",
        "", "MAE", "RMSE", "Spearman"
    );
    row(
        "tuned, on the films it was tuned on",
        &tuning.best_on_tune_half,
    );
    for (name, scores) in &tuning.check {
        row(&format!("{name}, on the check half"), scores);
    }
    println!("The check-half rows are the honest comparison: tuning never saw those films.");
    Ok(())
}

fn parse_region(code: &str) -> Result<Region, String> {
    Region::parse(code).ok_or_else(|| format!("\"{code}\" is not a two-letter country code"))
}
