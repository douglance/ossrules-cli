//! `ossrules patterns` — the taxonomy the corpus indexes techniques by.

use incurs::command::{TypedContext, TypedResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::corpus::{self, CorpusEnv, Pattern};
use crate::techniques::{self, TechniqueRow};

/// One catalog pattern with its usage count.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatternRow {
    /// Stable pattern id, as passed to `--pattern`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One-line description.
    pub summary: String,
    /// How the pattern shows up across the corpus.
    pub detail: String,
    /// Entries that demonstrate it.
    pub projects: usize,
    /// Techniques tagged with it.
    pub techniques: usize,
}

/// The whole catalog.
#[derive(Debug, JsonSchema, Serialize)]
pub struct PatternList {
    /// Patterns in the catalog.
    pub total: usize,
    /// Every pattern, most-used first.
    pub patterns: Vec<PatternRow>,
}

/// Runs `patterns list`.
pub async fn list(ctx: TypedContext<(), (), CorpusEnv>) -> TypedResult<PatternList> {
    let corpus = match corpus::resolve(&ctx.globals, &ctx.env) {
        Ok(corpus) => corpus,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
    let (patterns, projects) = match (corpus.patterns(), corpus.projects()) {
        (Ok(patterns), Ok(projects)) => (patterns, projects),
        (Err(error), _) | (_, Err(error)) => return TypedResult::error(error.code, error.message),
    };

    let mut rows: Vec<PatternRow> = patterns
        .into_iter()
        .map(|pattern| {
            let used_by = projects
                .iter()
                .filter(|project| project.patterns.contains(&pattern.id))
                .count();
            let tagged = projects
                .iter()
                .flat_map(|project| &project.techniques)
                .filter(|technique| technique.pattern.as_deref() == Some(pattern.id.as_str()))
                .count();
            PatternRow {
                id: pattern.id,
                name: pattern.name,
                summary: pattern.summary,
                detail: pattern.detail,
                projects: used_by,
                techniques: tagged,
            }
        })
        .collect();

    rows.sort_by(|a, b| b.projects.cmp(&a.projects).then(a.id.cmp(&b.id)));

    TypedResult::ok(PatternList {
        total: rows.len(),
        patterns: rows,
    })
}

/// Positional arguments for `patterns show`.
#[derive(Deserialize, incurs::Args)]
pub struct PatternArgs {
    /// Pattern id, as listed by `patterns list`.
    pub id: String,
}

/// Named options for `patterns show`.
#[derive(Deserialize, incurs::Options)]
pub struct ShowOptions {
    /// Maximum worked examples to include. `0` returns every one.
    #[incurs(alias = "n", default = 5)]
    pub examples: u32,
}

/// One pattern, with the entries and quotes that demonstrate it.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatternDetail {
    /// The catalog entry and its application guide.
    pub pattern: Pattern,
    /// Slugs of every entry demonstrating it.
    pub projects: Vec<String>,
    /// Techniques tagged with it, each with its verbatim quote.
    pub examples: Vec<TechniqueRow>,
    /// Techniques tagged with it in total, before `--examples`.
    pub tagged: usize,
}

/// Runs `patterns show`.
pub async fn show(
    ctx: TypedContext<PatternArgs, ShowOptions, CorpusEnv>,
) -> TypedResult<PatternDetail> {
    let corpus = match corpus::resolve(&ctx.globals, &ctx.env) {
        Ok(corpus) => corpus,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
    let (patterns, projects) = match (corpus.patterns(), corpus.projects()) {
        (Ok(patterns), Ok(projects)) => (patterns, projects),
        (Err(error), _) | (_, Err(error)) => return TypedResult::error(error.code, error.message),
    };

    let Some(pattern) = patterns.into_iter().find(|entry| entry.id == ctx.args.id) else {
        return TypedResult::error(
            "UNKNOWN_PATTERN",
            format!(
                "unknown pattern `{}`. Run `ossrules patterns list` to see every id.",
                ctx.args.id
            ),
        );
    };

    let slugs: Vec<String> = projects
        .iter()
        .filter(|project| project.patterns.contains(&pattern.id))
        .map(|project| project.slug.clone())
        .collect();

    let tagged: Vec<TechniqueRow> = projects
        .iter()
        .flat_map(techniques::rows)
        .filter(|row| row.pattern.as_deref() == Some(pattern.id.as_str()))
        .collect();

    let total = tagged.len();
    let examples = match ctx.options.examples {
        0 => tagged,
        limit => tagged.into_iter().take(limit as usize).collect(),
    };

    TypedResult::ok(PatternDetail {
        pattern,
        projects: slugs,
        examples,
        tagged: total,
    })
}
