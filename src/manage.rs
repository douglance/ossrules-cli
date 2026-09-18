//! `ossrules corpus` — where the data comes from, and how it gets there.

use std::path::{Path, PathBuf};

use incurs::command::{TypedContext, TypedResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::corpus::{self, Corpus, CorpusEnv, UPSTREAM};

/// What `corpus path` reports.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpusPath {
    /// The resolved checkout.
    pub root: String,
    /// How it was found: `explicit`, `environment`, `enclosing checkout`, or `managed clone`.
    pub origin: &'static str,
    /// Commit the checkout is on, when it is a Git working tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// Editorial entries present.
    pub projects: usize,
    /// Repositories with a skill snapshot.
    pub skill_repositories: usize,
    /// Where `corpus sync` would clone without `--dir`.
    pub managed: String,
}

/// Runs `corpus path`.
pub async fn path(ctx: TypedContext<(), (), CorpusEnv>) -> TypedResult<CorpusPath> {
    let corpus = match corpus::resolve(&ctx.globals, &ctx.env) {
        Ok(corpus) => corpus,
        Err(error) => return TypedResult::error(error.code, error.message),
    };

    let projects = corpus.projects().map(|list| list.len()).unwrap_or(0);
    let skill_repositories = corpus.skill_manifests().map(|list| list.len()).unwrap_or(0);

    TypedResult::ok(CorpusPath {
        root: corpus.root().display().to_string(),
        origin: corpus.origin,
        sha: head_sha(corpus.root()).await,
        projects,
        skill_repositories,
        managed: corpus::managed_root().display().to_string(),
    })
}

/// Named options for `corpus sync`.
#[derive(Deserialize, incurs::Options)]
pub struct SyncOptions {
    /// Where to clone or update. Defaults to the managed clone directory.
    pub dir: Option<String>,
    /// Branch to track.
    #[incurs(default = "main")]
    pub branch: String,
}

/// What `corpus sync` did.
#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOutput {
    /// The checkout that was cloned or updated.
    pub root: String,
    /// `cloned` or `updated`.
    pub action: &'static str,
    /// Commit the checkout is now on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// Editorial entries now present.
    pub projects: usize,
    /// Upstream the clone came from.
    pub upstream: &'static str,
}

/// Runs `corpus sync`.
///
/// A fresh directory gets a shallow clone; an existing one gets a fast-forward
/// pull. Fast-forward only on purpose: `--dir` can name a checkout somebody is
/// working in, and a sync command has no business discarding their commits.
pub async fn sync(ctx: TypedContext<(), SyncOptions, CorpusEnv>) -> TypedResult<SyncOutput> {
    let root = ctx
        .options
        .dir
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(corpus::managed_root);
    let branch = &ctx.options.branch;

    let existing = root.join(".git").exists();
    if !existing && root.exists() && root.read_dir().is_ok_and(|mut d| d.next().is_some()) {
        return TypedResult::error(
            "DIRECTORY_NOT_EMPTY",
            format!(
                "{} already has contents but is not a Git checkout. Remove it or choose another --dir.",
                root.display()
            ),
        );
    }

    let action = match existing {
        true => {
            let pull = git(&root, &["pull", "--ff-only", "origin", branch]).await;
            match pull {
                Ok(_) => "updated",
                Err(message) => {
                    return TypedResult::error(
                        "SYNC_FAILED",
                        format!("cannot fast-forward {}: {message}", root.display()),
                    );
                }
            }
        }
        false => {
            if let Some(parent) = root.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                return TypedResult::error(
                    "SYNC_FAILED",
                    format!("cannot create {}: {error}", parent.display()),
                );
            }
            let clone = git(
                Path::new("."),
                &[
                    "clone",
                    "--depth",
                    "1",
                    "--branch",
                    branch,
                    UPSTREAM,
                    &root.display().to_string(),
                ],
            )
            .await;
            match clone {
                Ok(_) => "cloned",
                Err(message) => {
                    return TypedResult::error(
                        "SYNC_FAILED",
                        format!("cannot clone {UPSTREAM}: {message}"),
                    );
                }
            }
        }
    };

    // Reopening proves the sync produced something the reader can actually use,
    // rather than reporting success because git exited zero.
    let projects = match Corpus::open(Some(&root.display().to_string())) {
        Ok(corpus) => match corpus.projects() {
            Ok(list) => list.len(),
            Err(error) => return TypedResult::error(error.code, error.message),
        },
        Err(error) => return TypedResult::error(error.code, error.message),
    };

    TypedResult::ok(SyncOutput {
        root: root.display().to_string(),
        action,
        sha: head_sha(&root).await,
        projects,
        upstream: UPSTREAM,
    })
}

/// Runs one git command, returning stdout or the failure.
async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .await
        .map_err(|error| format!("cannot run git: {error}"))?;

    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()),
        false => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

/// The commit a checkout is on, or `None` when it is not itself a Git working tree.
///
/// `git rev-parse` searches upwards, so asking a corpus that is not a checkout
/// returns whatever repository encloses it. Reporting that as the corpus commit
/// is worse than reporting nothing: it names a real sha that has nothing to do
/// with the data, and a caller comparing it against upstream would be comparing
/// against an unrelated project.
async fn head_sha(root: &Path) -> Option<String> {
    let toplevel = git(root, &["rev-parse", "--show-toplevel"]).await.ok()?;
    let owns_root = std::fs::canonicalize(&toplevel).ok()? == std::fs::canonicalize(root).ok()?;

    match owns_root {
        true => git(root, &["rev-parse", "HEAD"]).await.ok(),
        false => None,
    }
}
