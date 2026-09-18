//! `ossrules corpus` — where the data comes from, and how it gets there.

use std::fs;
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
        sha: head_sha(corpus.root()),
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
        sha: head_sha(&root),
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
/// Reads `.git` rather than running `git rev-parse`, for three reasons. It only
/// ever looks at this directory, where `rev-parse` searches upwards and would
/// report the commit of whatever repository encloses a corpus that is not one.
/// It leaves `corpus path` a pure local read, which is what the command claims
/// on every surface. And it does not hand a caller-named directory to a program
/// that reads executable configuration out of that same directory.
fn head_sha(root: &Path) -> Option<String> {
    let head = git_dir(root)?;
    let pointer = fs::read_to_string(head.join("HEAD")).ok()?;
    let pointer = pointer.trim();

    // A detached HEAD holds the commit; otherwise it names the ref that does.
    let Some(reference) = pointer.strip_prefix("ref:") else {
        return object_id(pointer);
    };
    let reference = reference.trim();

    if let Ok(loose) = fs::read_to_string(head.join(reference))
        && let Some(sha) = object_id(loose.trim())
    {
        return Some(sha);
    }

    // A ref with no loose file has been packed.
    let packed = fs::read_to_string(head.join("packed-refs")).ok()?;
    packed
        .lines()
        .filter(|line| !line.starts_with('#') && !line.starts_with('^'))
        .filter_map(|line| line.split_once(' '))
        .find(|(_, name)| name.trim() == reference)
        .and_then(|(sha, _)| object_id(sha))
}

/// The `.git` directory for a checkout, following the file form a worktree uses.
fn git_dir(root: &Path) -> Option<PathBuf> {
    let candidate = root.join(".git");

    if candidate.is_dir() {
        return Some(candidate);
    }

    // A linked worktree or submodule stores `gitdir: <path>` in a file instead.
    let pointer = fs::read_to_string(&candidate).ok()?;
    let target = PathBuf::from(pointer.strip_prefix("gitdir:")?.trim());

    let resolved = match target.is_absolute() {
        true => target,
        false => root.join(target),
    };
    resolved.is_dir().then_some(resolved)
}

/// One object id, or `None` when the text is not one.
///
/// Guards against reporting a stray file's contents as a commit. Both hash
/// lengths are accepted, since a SHA-256 repository is still a repository.
fn object_id(text: &str) -> Option<String> {
    let text = text.trim();
    let shaped = matches!(text.len(), 40 | 64) && text.chars().all(|c| c.is_ascii_hexdigit());

    shaped.then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::head_sha;
    use std::fs;
    use std::path::PathBuf;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    /// Builds a throwaway directory holding a handcrafted `.git`.
    ///
    /// Handcrafted rather than produced by `git init`, so these assert what the
    /// reader does with a layout on disk instead of re-testing git.
    fn checkout(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ossrules-head-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".git")).expect("create .git");
        root
    }

    /// HEAD naming a branch whose ref is a loose file.
    #[test]
    fn a_loose_ref_resolves() {
        let root = checkout("loose");
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        fs::write(root.join(".git/refs/heads/main"), format!("{SHA}\n")).unwrap();

        assert_eq!(head_sha(&root).as_deref(), Some(SHA));
    }

    /// The same branch after `git gc` has packed its ref away.
    #[test]
    fn a_packed_ref_resolves() {
        let root = checkout("packed");
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            root.join(".git/packed-refs"),
            format!(
                "# pack-refs with: peeled fully-peeled sorted \n{SHA} refs/heads/main\n^ffff\n"
            ),
        )
        .unwrap();

        assert_eq!(head_sha(&root).as_deref(), Some(SHA));
    }

    /// A detached HEAD holds the commit itself.
    #[test]
    fn a_detached_head_resolves() {
        let root = checkout("detached");
        fs::write(root.join(".git/HEAD"), format!("{SHA}\n")).unwrap();

        assert_eq!(head_sha(&root).as_deref(), Some(SHA));
    }

    /// A worktree stores `gitdir: <path>` in a file where `.git` would be.
    #[test]
    fn a_worktree_pointer_is_followed() {
        let root = std::env::temp_dir().join("ossrules-head-worktree");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("tree")).unwrap();
        fs::create_dir_all(root.join("real")).unwrap();
        fs::write(root.join("tree/.git"), "gitdir: ../real\n").unwrap();
        fs::write(root.join("real/HEAD"), format!("{SHA}\n")).unwrap();

        assert_eq!(head_sha(&root.join("tree")).as_deref(), Some(SHA));
    }

    /// A directory that is not a checkout reports nothing.
    ///
    /// This is the case the old `git rev-parse` call got wrong: it searched
    /// upwards and returned the enclosing repository's commit.
    #[test]
    fn a_plain_directory_reports_nothing() {
        let root = std::env::temp_dir().join("ossrules-head-plain");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        assert_eq!(head_sha(&root), None);
    }

    /// A ref that is not an object id is not reported as one.
    #[test]
    fn a_ref_holding_junk_is_refused() {
        let root = checkout("junk");
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        fs::write(root.join(".git/refs/heads/main"), "not-a-commit\n").unwrap();

        assert_eq!(head_sha(&root), None);
    }
}
