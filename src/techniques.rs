//! `ossrules techniques` — the observations, across every entry.
//!
//! An entry's techniques are the reusable part of the corpus: each one names a
//! move an instruction file makes and quotes the line that makes it. Listing
//! them across projects is how you find prior art for a rule you are about to
//! write, which is harder to do one entry at a time.

use incurs::command::{TypedContext, TypedResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::corpus::{self, CorpusEnv, Project};

/// Named options for `techniques list`.
#[derive(Deserialize, incurs::Options)]
pub struct ListOptions {
    /// Only techniques tagged with this pattern id.
    #[incurs(alias = "p")]
    pub pattern: Option<String>,
    /// Only techniques from this project slug.
    pub slug: Option<String>,
    /// Free text matched against title, analysis, and the quoted source.
    #[incurs(alias = "q")]
    pub query: Option<String>,
    /// Maximum rows to return. `0` returns every match.
    #[incurs(alias = "n", default = 0)]
    pub limit: u32,
}

/// One technique, carrying the project it was observed in.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TechniqueRow {
    /// Project slug.
    pub slug: String,
    /// Project display name.
    pub project: String,
    /// `owner/repo`.
    pub repository: String,
    /// What the instruction file does.
    pub title: String,
    /// Editorial analysis of the move.
    pub body: String,
    /// Verbatim excerpt from the pinned source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// File the quote came from.
    pub source_path: String,
    /// Pattern this technique is an instance of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// The quoted file at the commit it was read at.
    pub url: String,
}

/// Matching techniques and how many there were to match.
#[derive(Debug, JsonSchema, Serialize)]
pub struct TechniqueList {
    /// Techniques in the corpus.
    pub total: usize,
    /// Techniques passing the filters, before `--limit`.
    pub matched: usize,
    /// The returned rows.
    pub techniques: Vec<TechniqueRow>,
}

/// Flattens one project's techniques into rows.
pub fn rows(project: &Project) -> Vec<TechniqueRow> {
    project
        .techniques
        .iter()
        .map(|technique| {
            let source_path = technique
                .source_path
                .clone()
                .unwrap_or_else(|| project.instruction_path().to_string());
            TechniqueRow {
                slug: project.slug.clone(),
                project: project.name.clone(),
                repository: project.repository(),
                title: technique.title.clone(),
                body: technique.body.clone(),
                quote: technique.quote.clone(),
                url: format!(
                    "https://github.com/{}/blob/{}/{source_path}",
                    project.repository(),
                    project.last_commit.sha
                ),
                source_path,
                pattern: technique.pattern.clone(),
            }
        })
        .collect()
}

/// Runs `techniques list`.
pub async fn list(ctx: TypedContext<(), ListOptions, CorpusEnv>) -> TypedResult<TechniqueList> {
    let corpus = match corpus::resolve(&ctx.globals, &ctx.env) {
        Ok(corpus) => corpus,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
    let projects = match corpus.projects() {
        Ok(projects) => projects,
        Err(error) => return TypedResult::error(error.code, error.message),
    };

    let all: Vec<TechniqueRow> = projects.iter().flat_map(rows).collect();
    let options = &ctx.options;

    let matched: Vec<TechniqueRow> = all
        .into_iter()
        .filter(|row| match &options.slug {
            Some(slug) => &row.slug == slug,
            None => true,
        })
        .filter(|row| match &options.pattern {
            Some(pattern) => row.pattern.as_deref() == Some(pattern.as_str()),
            None => true,
        })
        .filter(|row| match &options.query {
            None => true,
            Some(query) => {
                corpus::contains(&row.title, query)
                    || corpus::contains(&row.body, query)
                    || row
                        .quote
                        .as_deref()
                        .is_some_and(|quote| corpus::contains(quote, query))
            }
        })
        .collect();

    // `total` counts the corpus, not the filtered set, so a caller can see how
    // selective a filter was without a second call.
    let total: usize = projects
        .iter()
        .map(|project| project.techniques.len())
        .sum();
    let count = matched.len();
    let techniques = match options.limit {
        0 => matched,
        limit => matched.into_iter().take(limit as usize).collect(),
    };

    TypedResult::ok(TechniqueList {
        total,
        matched: count,
        techniques,
    })
}
