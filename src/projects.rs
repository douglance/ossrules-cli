//! `ossrules projects` — the editorial corpus entries.

use incurs::command::{TypedContext, TypedResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::corpus::{self, Corpus, CorpusEnv, Project, Provenance, VendoredFile};

/// Named options for `projects list`.
#[derive(Deserialize, incurs::Options)]
pub struct ListOptions {
    /// Only projects in this language. Case-insensitive.
    #[incurs(alias = "l")]
    pub language: Option<String>,
    /// Only projects demonstrating this pattern id. Repeatable; every one must match.
    #[incurs(alias = "p")]
    #[serde(default)]
    pub pattern: Vec<String>,
    /// Only projects whose analyzed instruction file has this name, such as `CLAUDE.md`.
    pub instruction_file: Option<String>,
    /// Free text matched against name, repository, tagline, hook, and summary.
    #[incurs(alias = "q")]
    pub query: Option<String>,
    /// Sort order: `stars`, `name`, `slug`, `lines`, or `evaluated`.
    #[incurs(alias = "s", default = "stars")]
    pub sort: String,
    /// Maximum rows to return. `0` returns every match.
    #[incurs(alias = "n", default = 0)]
    pub limit: u32,
}

/// One row of `projects list`.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRow {
    /// Stable identifier, as passed to `projects show`.
    pub slug: String,
    /// Project display name.
    pub name: String,
    /// `owner/repo`.
    pub repository: String,
    /// Primary language.
    pub language: String,
    /// Star count at the corpus snapshot. Popularity, not a quality score.
    pub stars: u64,
    /// The analyzed instruction file.
    pub instruction_file: String,
    /// Lines in that file, measured from the pinned source.
    pub lines: u64,
    /// One line on what the instruction file does.
    pub hook: String,
    /// Pattern ids the entry demonstrates, comma-joined. `projects show` returns them as a list.
    pub patterns: String,
}

impl From<&Project> for ProjectRow {
    fn from(project: &Project) -> Self {
        Self {
            slug: project.slug.clone(),
            name: project.name.clone(),
            repository: project.repository(),
            language: project.language.clone(),
            stars: project.stars,
            instruction_file: project.instruction_path().to_string(),
            lines: project.file.lines,
            hook: project.hook.clone(),
            patterns: project.patterns.join(", "),
        }
    }
}

/// Matching entries and how many there were to match.
#[derive(Debug, JsonSchema, Serialize)]
pub struct ProjectList {
    /// Entries in the corpus.
    pub total: usize,
    /// Entries passing the filters, before `--limit`.
    pub matched: usize,
    /// The returned rows.
    pub projects: Vec<ProjectRow>,
}

/// Runs `projects list`.
pub async fn list(ctx: TypedContext<(), ListOptions, CorpusEnv>) -> TypedResult<ProjectList> {
    let corpus = match corpus::resolve(&ctx.globals, &ctx.env) {
        Ok(corpus) => corpus,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
    let all = match corpus.projects() {
        Ok(projects) => projects,
        Err(error) => return TypedResult::error(error.code, error.message),
    };

    let options = &ctx.options;
    let mut matched: Vec<&Project> = all.iter().filter(|p| keeps(p, options)).collect();

    match options.sort.as_str() {
        "name" => matched.sort_by_key(|project| project.name.to_lowercase()),
        "slug" => matched.sort_by(|a, b| a.slug.cmp(&b.slug)),
        "lines" => matched.sort_by_key(|project| std::cmp::Reverse(project.file.lines)),
        "evaluated" => {
            matched.sort_by(|a, b| b.evaluated_at.cmp(&a.evaluated_at));
        }
        "stars" => matched.sort_by_key(|project| std::cmp::Reverse(project.stars)),
        other => {
            return TypedResult::error(
                "UNKNOWN_SORT",
                format!("unknown sort `{other}`. Use stars, name, slug, lines, or evaluated."),
            );
        }
    }

    let count = matched.len();
    let rows = match options.limit {
        0 => matched.as_slice(),
        limit => &matched[..count.min(limit as usize)],
    };

    TypedResult::ok(ProjectList {
        total: all.len(),
        matched: count,
        projects: rows.iter().map(|p| ProjectRow::from(*p)).collect(),
    })
}

/// Whether one entry passes every filter that was given.
fn keeps(project: &Project, options: &ListOptions) -> bool {
    if let Some(language) = &options.language
        && !project.language.eq_ignore_ascii_case(language)
    {
        return false;
    }

    if let Some(file) = &options.instruction_file
        && !project.instruction_path().eq_ignore_ascii_case(file)
    {
        return false;
    }

    if !options
        .pattern
        .iter()
        .all(|wanted| project.patterns.iter().any(|id| id == wanted))
    {
        return false;
    }

    match &options.query {
        None => true,
        Some(query) => {
            [
                project.name.as_str(),
                project.slug.as_str(),
                project.language.as_str(),
                project.tagline.as_str(),
                project.hook.as_str(),
                project.summary.as_str(),
            ]
            .iter()
            .any(|field| corpus::contains(field, query))
                || corpus::contains(&project.repository(), query)
        }
    }
}

/// Positional arguments for the commands that name one project.
#[derive(Deserialize, incurs::Args)]
pub struct ProjectArgs {
    /// Project slug, as listed by `projects list`.
    pub slug: String,
}

/// One entry, with the links its provenance implies.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetail {
    /// The corpus entry.
    pub project: Project,
    /// Where the analyzed file lives, at the commit it was read at.
    pub provenance: Provenance,
    /// The entry's page on ossrules.md.
    pub site_url: String,
    /// Skills discovered in the repository, from the independent skill scan.
    pub skills: usize,
}

/// Runs `projects show`.
pub async fn show(ctx: TypedContext<ProjectArgs, (), CorpusEnv>) -> TypedResult<ProjectDetail> {
    let corpus = match corpus::resolve(&ctx.globals, &ctx.env) {
        Ok(corpus) => corpus,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
    let project = match corpus.project(&ctx.args.slug) {
        Ok(project) => project,
        Err(error) => return TypedResult::error(error.code, error.message),
    };

    let license = corpus
        .vendored_manifest(&project.slug)
        .ok()
        .and_then(|manifest| manifest.license);
    let provenance = Provenance::new(
        &project.repository(),
        &project.last_commit.sha,
        project.instruction_path(),
        license,
    );
    let skills = corpus
        .skill_manifest(&project.slug)
        .map(|manifest| manifest.skills.len())
        .unwrap_or(0);

    TypedResult::ok(ProjectDetail {
        site_url: format!("https://ossrules.md/{}/{}", project.owner, project.repo),
        provenance,
        skills,
        project,
    })
}

/// Named options for `projects source`.
#[derive(Deserialize, incurs::Options)]
pub struct SourceOptions {
    /// Repository-relative path to read. Defaults to the analyzed instruction file.
    pub path: Option<String>,
    /// List the stored files instead of reading one.
    #[incurs(alias = "l")]
    pub list: bool,
}

/// A stored copy of another project's documentation.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceOutput {
    /// Project slug.
    pub slug: String,
    /// Where the bytes came from.
    pub provenance: Provenance,
    /// The file that was read, when one was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Verbatim file contents, when a file was read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Every stored file, when `--list` was passed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<VendoredFile>,
}

/// Runs `projects source`.
pub async fn source(
    ctx: TypedContext<ProjectArgs, SourceOptions, CorpusEnv>,
) -> TypedResult<SourceOutput> {
    let corpus = match corpus::resolve(&ctx.globals, &ctx.env) {
        Ok(corpus) => corpus,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
    let project = match corpus.project(&ctx.args.slug) {
        Ok(project) => project,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
    let manifest = match corpus.vendored_manifest(&project.slug) {
        Ok(manifest) => manifest,
        Err(error) => return TypedResult::error(error.code, error.message),
    };

    let path = ctx
        .options
        .path
        .clone()
        .unwrap_or_else(|| project.instruction_path().to_string());

    // The manifest, not the filesystem, decides what exists: a path it does not
    // list is not part of this snapshot even if a file happens to sit there.
    if !ctx.options.list && !manifest.files.iter().any(|file| file.path == path) {
        return TypedResult::error(
            "NO_VENDORED_SOURCE",
            format!(
                "`{path}` is not stored for {}. Stored: {}.",
                project.slug,
                manifest
                    .files
                    .iter()
                    .map(|file| file.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }

    let provenance = Provenance::new(
        &project.repository(),
        &manifest.sha,
        &path,
        manifest.license.clone(),
    );

    if ctx.options.list {
        return TypedResult::ok(SourceOutput {
            slug: project.slug,
            provenance,
            path: None,
            content: None,
            files: manifest.files,
        });
    }

    match corpus_file(&corpus, &project.slug, &path) {
        Ok(content) => TypedResult::ok(SourceOutput {
            slug: project.slug,
            provenance,
            path: Some(path),
            content: Some(content),
            files: Vec::new(),
        }),
        Err(message) => TypedResult::error("NO_VENDORED_SOURCE", message),
    }
}

/// Reads one vendored file, flattening the error to a message.
fn corpus_file(corpus: &Corpus, slug: &str, path: &str) -> Result<String, String> {
    corpus
        .vendored_file(slug, path)
        .map_err(|error| error.message)
}
