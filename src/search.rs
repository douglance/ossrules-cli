//! `ossrules search` — one query across every part of the corpus.
//!
//! The separate list commands each take a `--query`, which means finding
//! everything about a subject takes three calls and knowing in advance which
//! one will have it. This is the entry point that does not require knowing.

use incurs::command::{TypedContext, TypedResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::corpus::{self, CorpusEnv, THIRD_PARTY_NOTICE};
use crate::techniques::{self, TechniqueRow};

/// Positional arguments for `search`.
#[derive(Deserialize, incurs::Args)]
pub struct SearchArgs {
    /// Text to look for. Case-insensitive substring match.
    pub query: String,
}

/// Named options for `search`.
#[derive(Deserialize, incurs::Options)]
pub struct SearchOptions {
    /// Maximum hits per section. `0` returns every hit.
    #[incurs(alias = "n", default = 10)]
    pub limit: u32,
}

/// One matching project.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectHit {
    /// Project slug, as passed to `projects show`.
    pub slug: String,
    /// Project display name.
    pub name: String,
    /// `owner/repo`.
    pub repository: String,
    /// One line on what the instruction file does.
    pub hook: String,
}

/// One matching skill.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillHit {
    /// Skill id, as passed to `skills show`.
    pub id: String,
    /// Skill name.
    pub name: String,
    /// Project slug.
    pub slug: String,
    /// Upstream description.
    pub description: String,
}

/// One matching pattern.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatternHit {
    /// Pattern id, as passed to `patterns show`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One-line description.
    pub summary: String,
}

/// What one query found, by section.
#[derive(Debug, JsonSchema, Serialize)]
pub struct SearchOutput {
    /// The query, echoed so a stored result is self-describing.
    pub query: String,
    /// Hits across all sections, before `--limit`.
    pub matched: usize,
    /// Matching patterns.
    pub patterns: Vec<PatternHit>,
    /// Matching entries.
    pub projects: Vec<ProjectHit>,
    /// Matching techniques, each with its verbatim quote.
    pub techniques: Vec<TechniqueRow>,
    /// Matching skills.
    pub skills: Vec<SkillHit>,
    /// That every `quote` and `description` below is another project's text.
    pub notice: &'static str,
}

/// Runs `search`.
pub async fn run(
    ctx: TypedContext<SearchArgs, SearchOptions, CorpusEnv>,
) -> TypedResult<SearchOutput> {
    let corpus = match corpus::resolve(&ctx.globals, &ctx.env) {
        Ok(corpus) => corpus,
        Err(error) => return TypedResult::error(error.code, error.message),
    };

    let query = ctx.args.query.trim();
    if query.is_empty() {
        return TypedResult::error("EMPTY_QUERY", "a search needs a non-empty query.");
    }

    let projects = match corpus.projects() {
        Ok(projects) => projects,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
    let patterns = match corpus.patterns() {
        Ok(patterns) => patterns,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
    let manifests = match corpus.skill_manifests() {
        Ok(manifests) => manifests,
        Err(error) => return TypedResult::error(error.code, error.message),
    };

    let pattern_hits: Vec<PatternHit> = patterns
        .into_iter()
        .filter(|pattern| {
            corpus::contains(&pattern.id, query)
                || corpus::contains(&pattern.name, query)
                || corpus::contains(&pattern.summary, query)
                || corpus::contains(&pattern.detail, query)
        })
        .map(|pattern| PatternHit {
            id: pattern.id,
            name: pattern.name,
            summary: pattern.summary,
        })
        .collect();

    let project_hits: Vec<ProjectHit> = projects
        .iter()
        .filter(|project| {
            corpus::contains(&project.name, query)
                || corpus::contains(&project.slug, query)
                || corpus::contains(&project.repository(), query)
                || corpus::contains(&project.tagline, query)
                || corpus::contains(&project.hook, query)
                || corpus::contains(&project.summary, query)
        })
        .map(|project| ProjectHit {
            slug: project.slug.clone(),
            name: project.name.clone(),
            repository: project.repository(),
            hook: project.hook.clone(),
        })
        .collect();

    let technique_hits: Vec<TechniqueRow> = projects
        .iter()
        .flat_map(techniques::rows)
        .filter(|row| {
            corpus::contains(&row.title, query)
                || corpus::contains(&row.body, query)
                || row
                    .quote
                    .as_deref()
                    .is_some_and(|quote| corpus::contains(quote, query))
        })
        .collect();

    let skill_hits: Vec<SkillHit> = manifests
        .iter()
        .flat_map(|manifest| {
            manifest
                .skills
                .iter()
                .map(move |skill| (manifest.slug.as_str(), skill))
        })
        .filter(|(_, skill)| {
            corpus::contains(&skill.name, query)
                || corpus::contains(&skill.description, query)
                || corpus::contains(&skill.path, query)
        })
        .map(|(slug, skill)| SkillHit {
            id: skill.id.clone(),
            name: skill.name.clone(),
            slug: slug.to_string(),
            description: skill.description.clone(),
        })
        .collect();

    let matched = pattern_hits.len() + project_hits.len() + technique_hits.len() + skill_hits.len();

    let limit = ctx.options.limit;

    TypedResult::ok(SearchOutput {
        query: query.to_string(),
        matched,
        patterns: cap(pattern_hits, limit),
        projects: cap(project_hits, limit),
        techniques: cap(technique_hits, limit),
        skills: cap(skill_hits, limit),
        notice: THIRD_PARTY_NOTICE,
    })
}

/// Caps one section, where `0` means no cap.
fn cap<T>(mut hits: Vec<T>, limit: u32) -> Vec<T> {
    if limit > 0 {
        hits.truncate(limit as usize);
    }
    hits
}
