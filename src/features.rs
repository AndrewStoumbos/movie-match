//! Turns a movie's TMDB details into labeled, weighted feature tokens.
//!
//! Each token is prefixed with its field (`genre:878`, `dir:137427`) so the same id
//! in different fields stays distinct. A token's weight here is only its field's
//! importance; rarity (IDF) is applied later in [`crate::tfidf`].

use std::collections::BTreeMap;

use crate::tmdb::MovieDetails;

/// Feature tokens mapped to weights. Ordered, so sums over it are deterministic.
pub type Features = BTreeMap<String, f32>;

/// Crew jobs that count as writing credits.
const WRITER_JOBS: [&str; 2] = ["Screenplay", "Writer"];

/// How much a match in each field counts, before rarity is taken into account.
#[derive(Debug, Clone, Copy)]
pub struct FieldWeights {
    pub genre: f32,
    pub keyword: f32,
    /// Keywords that describe a film's mood rather than its content (see [`TONE_KEYWORDS`]).
    pub tone: f32,
    pub director: f32,
    pub writer: f32,
    /// Weight of the top-billed actor; the actor billed n-th gets `cast / n`.
    pub cast: f32,
    /// How many top-billed actors to include.
    pub cast_limit: usize,
}

impl Default for FieldWeights {
    fn default() -> Self {
        Self {
            genre: 2.0,
            keyword: 1.5,
            tone: 0.4,
            director: 3.0,
            writer: 2.0,
            cast: 2.0,
            cast_limit: 5,
        }
    }
}

/// TMDB mood tags ("aggressive", "farcical", …). They're rare, so rarity weighting
/// would otherwise make two films that merely share a tone look alike.
pub const TONE_KEYWORDS: &[&str] = &[
    "absurd",
    "admiring",
    "adoring",
    "aggressive",
    "amused",
    "angry",
    "anxious",
    "appreciative",
    "approving",
    "audacious",
    "awestruck",
    "bewildered",
    "bold",
    "callous",
    "candid",
    "cautionary",
    "celebratory",
    "cheerful",
    "comforting",
    "compassionate",
    "complicated",
    "condescending",
    "critical",
    "cruel",
    "defiant",
    "demeaning",
    "depressing",
    "derisive",
    "disdainful",
    "disheartening",
    "dramatic",
    "earnest",
    "empathetic",
    "enchanting",
    "enthusiastic",
    "exhilarated",
    "familiar",
    "farcical",
    "frightened",
    "grim",
    "harsh",
    "hilarious",
    "hopeful",
    "horrified",
    "humorous",
    "inspirational",
    "intense",
    "introspective",
    "joyful",
    "joyous",
    "lighthearted",
    "melancholy",
    "melodramatic",
    "mean spirited",
    "mocking",
    "nostalgic",
    "optimistic",
    "playful",
    "powerful",
    "provocative",
    "reflective",
    "sad",
    "sarcastic",
    "sentimental",
    "serious",
    "shocking",
    "sincere",
    "somber",
    "suspenseful",
    "sympathetic",
    "tense",
    "thoughtful",
    "thrilling",
    "tragic",
    "urgent",
    "vibrant",
    "whimsical",
    "witty",
    "wry",
    "zany",
];

fn is_tone_keyword(name: &str) -> bool {
    TONE_KEYWORDS.contains(&name.trim().to_lowercase().as_str())
}

struct Token {
    key: String,
    weight: f32,
    label: String,
}

/// Feature tokens for `movie`, each weighted by its field's importance.
pub fn extract(movie: &MovieDetails, weights: &FieldWeights) -> Features {
    let mut features = Features::new();
    for token in tokens(movie, weights) {
        // A person can hold the same job twice (e.g. "Screenplay" and "Writer"); keep the max.
        let entry = features.entry(token.key).or_insert(0.0);
        *entry = entry.max(token.weight);
    }
    features
}

/// Drops the credits [`extract`] never reads (crew other than directors and writers,
/// cast past `weights.cast_limit`), so holding many movies takes far less memory.
/// Features and labels come out the same.
pub fn trim_to_used(movie: &mut MovieDetails, weights: &FieldWeights) {
    trim_keeping_cast(movie, weights, weights.cast_limit);
}

/// Like [`trim_to_used`], but keeps the top `max(weights.cast_limit, cast_limit)`
/// billed actors, for callers that also read cast past `weights.cast_limit`
/// (e.g. [`crate::quality::credits`] with a larger `cast_limit`).
pub fn trim_keeping_cast(movie: &mut MovieDetails, weights: &FieldWeights, cast_limit: usize) {
    movie
        .credits
        .crew
        .retain(|c| c.job == "Director" || WRITER_JOBS.contains(&c.job.as_str()));
    movie.credits.cast.sort_by_key(|actor| actor.order);
    movie
        .credits
        .cast
        .truncate(weights.cast_limit.max(cast_limit));
}

/// Human-readable names for `movie`'s tokens, e.g. `dir:137427` → `director Denis Villeneuve`.
pub fn labels(movie: &MovieDetails, weights: &FieldWeights) -> BTreeMap<String, String> {
    tokens(movie, weights)
        .into_iter()
        .map(|token| (token.key, token.label))
        .collect()
}

fn tokens(movie: &MovieDetails, weights: &FieldWeights) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut add = |key: String, weight: f32, label: String| {
        tokens.push(Token { key, weight, label });
    };

    for genre in &movie.genres {
        add(
            format!("genre:{}", genre.id),
            weights.genre,
            format!("genre {}", genre.name),
        );
    }
    for keyword in &movie.keywords.keywords {
        let (weight, kind) = if is_tone_keyword(&keyword.name) {
            (weights.tone, "tone")
        } else {
            (weights.keyword, "keyword")
        };
        add(
            format!("kw:{}", keyword.id),
            weight,
            format!("{kind} \"{}\"", keyword.name),
        );
    }
    for person in movie.crew_with_job("Director") {
        add(
            format!("dir:{}", person.id),
            weights.director,
            format!("director {}", person.name),
        );
    }
    for job in WRITER_JOBS {
        for person in movie.crew_with_job(job) {
            add(
                format!("writer:{}", person.id),
                weights.writer,
                format!("writer {}", person.name),
            );
        }
    }

    let mut cast: Vec<_> = movie.credits.cast.iter().collect();
    cast.sort_by_key(|actor| actor.order);
    for (i, actor) in cast.iter().take(weights.cast_limit).enumerate() {
        add(
            format!("cast:{}", actor.id),
            weights.cast / (i as f32 + 1.0),
            format!("actor {}", actor.name),
        );
    }

    tokens
}
