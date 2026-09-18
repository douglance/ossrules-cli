//! `ossrules stats` — totals derived from the dataset.
//!
//! Every number here is counted from the checkout at call time rather than
//! stored, and `scope` says what was counted. A total that nobody has to
//! remember to update cannot drift from the corpus it describes.

use std::collections::BTreeMap;

use incurs::command::{TypedContext, TypedResult};
use schemars::JsonSchema;
use serde::Serialize;

use crate::corpus::{self, CorpusEnv};

/// A count per group.
#[derive(Debug, JsonSchema, Serialize)]
pub struct Tally {
    /// The group.
    pub name: String,
    /// Entries in it.
    pub projects: usize,
}

/// The spread of a measured value across the corpus.
#[derive(Debug, JsonSchema, Serialize)]
pub struct Spread {
    /// Smallest value.
    pub min: u64,
    /// Middle value after sorting. With an even count, the upper of the two.
    pub median: u64,
    /// Largest value.
    pub max: u64,
    /// Sum across every entry.
    pub total: u64,
}

impl Spread {
    /// Summarizes a set of measurements.
    fn of(mut values: Vec<u64>) -> Self {
        values.sort_unstable();
        Self {
            min: values.first().copied().unwrap_or(0),
            median: values.get(values.len() / 2).copied().unwrap_or(0),
            max: values.last().copied().unwrap_or(0),
            total: values.iter().sum(),
        }
    }
}

/// Corpus totals.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// Editorial entries.
    pub projects: usize,
    /// Techniques across those entries.
    pub techniques: usize,
    /// Patterns in the catalog.
    pub patterns: usize,
    /// Repositories with a skill discovery snapshot.
    pub skill_repositories: usize,
    /// Skills discovered across them.
    pub skills: usize,
    /// Entries per analyzed instruction file name.
    pub instruction_files: Vec<Tally>,
    /// Entries per primary language, most first.
    pub languages: Vec<Tally>,
    /// Lines in the analyzed instruction files.
    pub lines: Spread,
    /// Bytes in the analyzed instruction files.
    pub bytes: Spread,
    /// Earliest review date among the entries.
    pub evaluated_from: String,
    /// Latest review date among the entries.
    pub evaluated_to: String,
    /// What these numbers count.
    pub scope: &'static str,
}

/// Runs `stats`.
pub async fn run(ctx: TypedContext<(), (), CorpusEnv>) -> TypedResult<Stats> {
    let corpus = match corpus::resolve(&ctx.globals, &ctx.env) {
        Ok(corpus) => corpus,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
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

    if projects.is_empty() {
        return TypedResult::error("CORPUS_EMPTY", "this checkout has no corpus entries.");
    }

    let mut languages: BTreeMap<&str, usize> = BTreeMap::new();
    let mut instruction_files: BTreeMap<&str, usize> = BTreeMap::new();
    for project in &projects {
        *languages.entry(project.language.as_str()).or_default() += 1;
        *instruction_files
            .entry(project.instruction_path())
            .or_default() += 1;
    }

    let mut dates: Vec<&str> = projects
        .iter()
        .map(|project| project.evaluated_at.as_str())
        .collect();
    dates.sort_unstable();

    TypedResult::ok(Stats {
        projects: projects.len(),
        techniques: projects
            .iter()
            .map(|project| project.techniques.len())
            .sum(),
        patterns: patterns.len(),
        skill_repositories: manifests.len(),
        skills: manifests.iter().map(|manifest| manifest.skills.len()).sum(),
        instruction_files: ranked(instruction_files),
        languages: ranked(languages),
        lines: Spread::of(projects.iter().map(|p| p.file.lines).collect()),
        bytes: Spread::of(projects.iter().map(|p| p.file.bytes).collect()),
        evaluated_from: dates.first().unwrap_or(&"").to_string(),
        evaluated_to: dates.last().unwrap_or(&"").to_string(),
        scope: "Counted from this checkout. Editorial entries are curated, not a \
                census of open source; skill snapshots are scanned separately and \
                may be newer than the instruction analysis for the same project.",
    })
}

/// Orders a tally by count, then name, so the output is stable.
fn ranked(counts: BTreeMap<&str, usize>) -> Vec<Tally> {
    let mut tallies: Vec<Tally> = counts
        .into_iter()
        .map(|(name, projects)| Tally {
            name: name.to_string(),
            projects,
        })
        .collect();
    tallies.sort_by(|a, b| b.projects.cmp(&a.projects).then(a.name.cmp(&b.name)));
    tallies
}
