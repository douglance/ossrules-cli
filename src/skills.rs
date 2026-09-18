//! `ossrules skills` — the discovered `SKILL.md` corpus.
//!
//! Skills are scanned independently of the editorial entries, at each
//! repository's current default branch, so a skill snapshot can be newer than
//! the instruction analysis for the same project. Nothing here implies a
//! discovered skill is referenced by that project's AGENTS.md.

use incurs::command::{TypedContext, TypedResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::corpus::{self, Corpus, CorpusEnv, CorpusError, Provenance, Skill, SkillFile};

/// Named options for `skills list`.
#[derive(Deserialize, incurs::Options)]
pub struct ListOptions {
    /// Only skills from this project slug.
    pub slug: Option<String>,
    /// Only skills declaring this tag. Repeatable; every one must match.
    #[incurs(alias = "t")]
    #[serde(default)]
    pub tag: Vec<String>,
    /// Free text matched against name, description, and path.
    #[incurs(alias = "q")]
    pub query: Option<String>,
    /// Maximum rows to return. `0` returns every match.
    #[incurs(alias = "n", default = 0)]
    pub limit: u32,
}

/// One row of `skills list`.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRow {
    /// Stable skill id, as passed to `skills show`.
    pub id: String,
    /// Skill name from upstream frontmatter. Names repeat across repositories.
    pub name: String,
    /// Project slug.
    pub slug: String,
    /// `owner/repo`.
    pub repository: String,
    /// Repository-relative path of the `SKILL.md`.
    pub path: String,
    /// Upstream description. Not an editorial review.
    pub description: String,
    /// Declared tags, comma-joined. `skills show` returns them as a list.
    pub tags: String,
    /// Files bundled with the skill.
    pub files: usize,
}

/// Matching skills and how many there were to match.
#[derive(Debug, JsonSchema, Serialize)]
pub struct SkillList {
    /// Skills in the corpus.
    pub total: usize,
    /// Skills passing the filters, before `--limit`.
    pub matched: usize,
    /// The returned rows.
    pub skills: Vec<SkillRow>,
}

/// Every skill in the corpus, paired with the repository that holds it.
fn every_skill(corpus: &Corpus) -> Result<Vec<(String, String, String, Skill)>, CorpusError> {
    Ok(corpus
        .skill_manifests()?
        .into_iter()
        .flat_map(|manifest| {
            manifest.skills.into_iter().map(move |skill| {
                (
                    manifest.slug.clone(),
                    manifest.repository.clone(),
                    manifest.sha.clone(),
                    skill,
                )
            })
        })
        .collect())
}

/// Runs `skills list`.
pub async fn list(ctx: TypedContext<(), ListOptions, CorpusEnv>) -> TypedResult<SkillList> {
    let corpus = match corpus::resolve(&ctx.globals, &ctx.env) {
        Ok(corpus) => corpus,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
    let all = match every_skill(&corpus) {
        Ok(skills) => skills,
        Err(error) => return TypedResult::error(error.code, error.message),
    };

    let options = &ctx.options;
    let matched: Vec<SkillRow> = all
        .iter()
        .filter(|(slug, ..)| match &options.slug {
            Some(wanted) => slug == wanted,
            None => true,
        })
        .filter(|(.., skill)| {
            options.tag.iter().all(|wanted| {
                skill
                    .tags
                    .iter()
                    .any(|tag| tag.eq_ignore_ascii_case(wanted))
            })
        })
        .filter(|(.., skill)| match &options.query {
            None => true,
            Some(query) => {
                corpus::contains(&skill.name, query)
                    || corpus::contains(&skill.description, query)
                    || corpus::contains(&skill.path, query)
            }
        })
        .map(|(slug, repository, _, skill)| SkillRow {
            id: skill.id.clone(),
            name: skill.name.clone(),
            slug: slug.clone(),
            repository: repository.clone(),
            path: skill.path.clone(),
            description: skill.description.clone(),
            tags: skill.tags.join(", "),
            files: skill.files.len(),
        })
        .collect();

    let count = matched.len();
    let skills = match options.limit {
        0 => matched,
        limit => matched.into_iter().take(limit as usize).collect(),
    };

    TypedResult::ok(SkillList {
        total: all.len(),
        matched: count,
        skills,
    })
}

/// Positional arguments for the commands that name one skill.
#[derive(Deserialize, incurs::Args)]
pub struct SkillArgs {
    /// Skill id, as listed by `skills list`.
    pub id: String,
}

/// Named options for `skills show`.
#[derive(Deserialize, incurs::Options)]
pub struct ShowOptions {
    /// Omit the `SKILL.md` body and return only metadata.
    pub metadata_only: bool,
}

/// One skill, with its source.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetail {
    /// The discovery record.
    pub skill: Skill,
    /// Project slug.
    pub slug: String,
    /// Where the bytes came from.
    pub provenance: Provenance,
    /// Verbatim `SKILL.md`, unless `--metadata-only` was passed or it is not stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Runs `skills show`.
pub async fn show(
    ctx: TypedContext<SkillArgs, ShowOptions, CorpusEnv>,
) -> TypedResult<SkillDetail> {
    let corpus = match corpus::resolve(&ctx.globals, &ctx.env) {
        Ok(corpus) => corpus,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
    let (slug, repository, sha, skill) = match find(&corpus, &ctx.args.id) {
        Ok(found) => found,
        Err(result) => return result,
    };

    let provenance = Provenance::new(&repository, &sha, &skill.path, skill.license.clone());

    // The bundle stores `SKILL.md` under its own name, so the body is one blob
    // lookup rather than a second scan of the repository.
    let content = match ctx.options.metadata_only {
        true => None,
        false => skill
            .files
            .iter()
            .find(|file| file.path == "SKILL.md" && file.text)
            .and_then(|file| corpus.skill_blob(&slug, &file.blob).ok()),
    };

    TypedResult::ok(SkillDetail {
        skill,
        slug,
        provenance,
        content,
    })
}

/// Positional arguments for `skills file`.
#[derive(Deserialize, incurs::Args)]
pub struct FileArgs {
    /// Skill id, as listed by `skills list`.
    pub id: String,
    /// Path of the bundled file, relative to the skill directory.
    pub path: String,
}

/// One file bundled with a skill.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillFileOutput {
    /// Skill id.
    pub id: String,
    /// The requested path.
    pub path: String,
    /// Where the bytes came from.
    pub provenance: Provenance,
    /// Verbatim file contents.
    pub content: String,
    /// Every file in the bundle, so a wrong path is one call from the right one.
    pub bundle: Vec<SkillFile>,
}

/// Runs `skills file`.
pub async fn file(ctx: TypedContext<FileArgs, (), CorpusEnv>) -> TypedResult<SkillFileOutput> {
    let corpus = match corpus::resolve(&ctx.globals, &ctx.env) {
        Ok(corpus) => corpus,
        Err(error) => return TypedResult::error(error.code, error.message),
    };
    let (slug, repository, sha, skill) = match find(&corpus, &ctx.args.id) {
        Ok(found) => found,
        Err(result) => return result,
    };

    let Some(entry) = skill.files.iter().find(|file| file.path == ctx.args.path) else {
        return TypedResult::error(
            "NO_SKILL_FILE",
            format!(
                "`{}` is not in this bundle. Bundled: {}.",
                ctx.args.path,
                skill
                    .files
                    .iter()
                    .map(|file| file.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    };

    if let Some(reason) = &entry.omitted {
        return TypedResult::error(
            "FILE_OMITTED",
            format!("`{}` was not stored: {reason}", ctx.args.path),
        );
    }

    if !entry.text {
        return TypedResult::error(
            "FILE_NOT_TEXT",
            format!(
                "`{}` is binary ({} bytes). Read it from {} instead.",
                ctx.args.path,
                entry.bytes,
                format_args!("https://github.com/{repository}/blob/{sha}/{}", skill.path)
            ),
        );
    }

    let content = match corpus.skill_blob(&slug, &entry.blob) {
        Ok(content) => content,
        Err(error) => return TypedResult::error(error.code, error.message),
    };

    // A bundled file's repository path is the skill directory plus its own.
    let directory = skill
        .path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    let repository_path = match directory.is_empty() {
        true => ctx.args.path.clone(),
        false => format!("{directory}/{}", ctx.args.path),
    };

    TypedResult::ok(SkillFileOutput {
        id: ctx.args.id,
        path: ctx.args.path,
        provenance: Provenance::new(&repository, &sha, &repository_path, skill.license.clone()),
        content,
        bundle: skill.files,
    })
}

/// Finds one skill by id, or the error a caller should see.
fn find<T>(corpus: &Corpus, id: &str) -> Result<(String, String, String, Skill), TypedResult<T>> {
    let all = match every_skill(corpus) {
        Ok(all) => all,
        Err(error) => return Err(TypedResult::error(error.code, error.message)),
    };

    match all.into_iter().find(|(.., skill)| skill.id == id) {
        Some(found) => Ok(found),
        None => Err(TypedResult::error(
            "UNKNOWN_SKILL",
            format!(
                "no skill with id `{id}`. Run `ossrules skills list --query <name>` to find one."
            ),
        )),
    }
}
