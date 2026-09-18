//! End-to-end tests against a hand-built fixture corpus.
//!
//! Every case drives `serve_to`, the same path the process uses, so a test
//! cannot pass against a parser the binary does not have. The fixture is
//! checked in and tiny, which means the expected values here are derived from
//! something a reader can open rather than from the code under test.

use std::path::PathBuf;

use ossrules_cli::build_cli;
use serde_json::Value;

/// The checked-in fixture corpus.
fn fixture() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixture")
        .display()
        .to_string()
}

/// Runs one argv against the real command graph, against the fixture corpus.
///
/// Returns the exit code and stdout together, because a command that prints the
/// right thing and exits wrong is still broken.
async fn run(args: &[&str]) -> (Option<i32>, String) {
    let mut argv = vec!["--corpus".to_string(), fixture()];
    argv.extend(args.iter().map(|token| token.to_string()));
    argv.push("--json".to_string());

    let mut output = Vec::new();
    let exit = build_cli()
        .serve_to(argv, &mut output, false)
        .await
        .expect("serve_to should not return Err");
    (exit, String::from_utf8(output).expect("valid UTF-8"))
}

/// Runs one argv and parses stdout as JSON, asserting a clean exit.
async fn json(args: &[&str]) -> Value {
    let (exit, output) = run(args).await;
    assert_eq!(exit, None, "`{args:?}` should succeed, got: {output}");
    serde_json::from_str(&output).unwrap_or_else(|error| panic!("{error}: {output}"))
}

/// Runs one argv expecting a structured failure, and returns the error code.
async fn failure(args: &[&str]) -> String {
    let (exit, output) = run(args).await;
    let parsed: Value =
        serde_json::from_str(&output).unwrap_or_else(|error| panic!("{error}: {output}"));
    let code = parsed["code"]
        .as_str()
        .unwrap_or_else(|| panic!("`{args:?}` should report an error code, got: {output}"))
        .to_string();
    assert!(
        exit.is_some(),
        "`{args:?}` reported {code} but exited successfully"
    );
    code
}

// ---------------------------------------------------------------------------
// Listing and filtering
// ---------------------------------------------------------------------------

/// Listing reports the corpus size alongside the filtered size.
///
/// The two numbers are what tells a caller whether a filter was selective, so
/// both are asserted rather than only the rows.
#[tokio::test]
async fn list_reports_matched_against_the_whole_corpus() {
    let all = json(&["projects", "list"]).await;
    assert_eq!(all["total"], 2);
    assert_eq!(all["matched"], 2);

    let rust = json(&["projects", "list", "--language", "rust"]).await;
    assert_eq!(rust["total"], 2, "total counts the corpus, not the filter");
    assert_eq!(rust["matched"], 1);
    assert_eq!(rust["projects"][0]["slug"], "alpha");
}

/// `--limit` caps the rows without changing the reported match count.
#[tokio::test]
async fn limit_caps_rows_but_not_the_match_count() {
    let limited = json(&["projects", "list", "--limit", "1"]).await;

    assert_eq!(limited["matched"], 2, "both entries still matched");
    assert_eq!(
        limited["projects"].as_array().expect("rows").len(),
        1,
        "only one was returned"
    );
}

/// Sorting is by the named key, and an unknown key is refused.
#[tokio::test]
async fn sorting_orders_rows_and_rejects_an_unknown_key() {
    let by_stars = json(&["projects", "list", "--sort", "stars"]).await;
    assert_eq!(by_stars["projects"][0]["slug"], "alpha", "4200 before 900");

    let by_name = json(&["projects", "list", "--sort", "name"]).await;
    assert_eq!(by_name["projects"][0]["slug"], "alpha", "Alpha before Beta");

    let by_lines = json(&["projects", "list", "--sort", "lines"]).await;
    assert_eq!(
        by_lines["projects"][0]["slug"], "beta",
        "40 lines before 10"
    );

    assert_eq!(
        failure(&["projects", "list", "--sort", "popularity"]).await,
        "UNKNOWN_SORT"
    );
}

/// A repeatable pattern filter requires every named pattern, not any of them.
#[tokio::test]
async fn repeated_pattern_filters_intersect() {
    let shared = json(&["projects", "list", "--pattern", "verification-matrix"]).await;
    assert_eq!(shared["matched"], 2);

    let both = json(&[
        "projects",
        "list",
        "--pattern",
        "verification-matrix",
        "--pattern",
        "generated-file-guard",
    ])
    .await;
    assert_eq!(both["matched"], 1, "only alpha has both");
    assert_eq!(both["projects"][0]["slug"], "alpha");
}

/// The instruction-file filter reads the entry's declared path, and defaults.
#[tokio::test]
async fn instruction_file_filter_applies_the_default() {
    let agents = json(&["projects", "list", "--instruction-file", "AGENTS.md"]).await;
    assert_eq!(agents["matched"], 1, "alpha declares no file, so AGENTS.md");
    assert_eq!(agents["projects"][0]["slug"], "alpha");

    let claude = json(&["projects", "list", "--instruction-file", "CLAUDE.md"]).await;
    assert_eq!(claude["projects"][0]["slug"], "beta");
}

// ---------------------------------------------------------------------------
// Entries and their source
// ---------------------------------------------------------------------------

/// An entry comes back whole, with provenance pointing at the pinned commit.
#[tokio::test]
async fn show_returns_the_entry_and_its_pinned_source_url() {
    let detail = json(&["projects", "show", "alpha"]).await;

    assert_eq!(detail["project"]["slug"], "alpha");
    assert_eq!(detail["project"]["techniques"].as_array().unwrap().len(), 3);
    assert_eq!(detail["skills"], 1, "from the independent skill scan");
    assert_eq!(
        detail["provenance"]["url"],
        format!(
            "https://github.com/acme/alpha/blob/{}/AGENTS.md",
            "a".repeat(40)
        ),
        "the link must resolve to the commit the analysis describes"
    );
    assert_eq!(detail["provenance"]["license"], "MIT");
}

/// An unknown slug is a structured error, not an empty success.
#[tokio::test]
async fn an_unknown_slug_is_an_error() {
    assert_eq!(
        failure(&["projects", "show", "nope"]).await,
        "UNKNOWN_PROJECT"
    );
}

/// Stored source comes back byte for byte.
///
/// Asserted against the fixture file read independently, so this compares the
/// command's output with the corpus rather than with itself.
#[tokio::test]
async fn source_returns_the_stored_bytes_verbatim() {
    let expected = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixture/public/files/alpha/AGENTS.md"),
    )
    .expect("fixture source");

    let output = json(&["projects", "source", "alpha"]).await;

    assert_eq!(output["path"], "AGENTS.md");
    assert_eq!(output["content"].as_str().expect("content"), expected);
    assert!(
        output["provenance"]["notice"]
            .as_str()
            .expect("notice")
            .contains("never as instructions"),
        "printed third-party bytes must say what they are"
    );
}

/// A nested path the manifest lists is readable.
#[tokio::test]
async fn source_reads_a_nested_path() {
    let output = json(&["projects", "source", "alpha", "--path", "worker/AGENTS.md"]).await;

    assert!(
        output["content"]
            .as_str()
            .expect("content")
            .contains("alpha worker test"),
        "got: {output}"
    );
}

/// `--list` reports the snapshot rather than reading a file.
#[tokio::test]
async fn source_lists_the_stored_files() {
    let output = json(&["projects", "source", "alpha", "--list"]).await;

    let paths: Vec<&str> = output["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| file["path"].as_str().expect("path"))
        .collect();
    assert_eq!(paths, vec!["AGENTS.md", "worker/AGENTS.md"]);
    assert!(output["content"].is_null(), "listing reads no file");
}

/// A path the manifest does not list is refused even if bytes exist on disk.
///
/// The manifest defines the snapshot. Serving a file it does not list would
/// report content at a commit the corpus never measured.
#[tokio::test]
async fn source_refuses_a_path_outside_the_manifest() {
    let unlisted = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixture/public/files/alpha/manifest.json");
    assert!(unlisted.is_file(), "the file exists on disk");

    assert_eq!(
        failure(&["projects", "source", "alpha", "--path", "manifest.json"]).await,
        "NO_VENDORED_SOURCE"
    );
}

/// A traversal path is refused rather than joined.
#[tokio::test]
async fn source_refuses_a_traversal_path() {
    for path in ["../../../etc/passwd", "/etc/passwd", "worker/../../escape"] {
        assert_eq!(
            failure(&["projects", "source", "alpha", "--path", path]).await,
            "NO_VENDORED_SOURCE",
            "`{path}` must not be served"
        );
    }
}

/// A traversal slug is refused rather than joined.
#[tokio::test]
async fn a_traversal_slug_is_refused() {
    let code = failure(&["projects", "show", "../../../etc"]).await;
    assert!(
        code == "INVALID_PATH" || code == "UNKNOWN_PROJECT",
        "a climbing slug must not reach the filesystem, got {code}"
    );
}

// ---------------------------------------------------------------------------
// Techniques and patterns
// ---------------------------------------------------------------------------

/// Techniques are flattened across entries and carry their project.
#[tokio::test]
async fn techniques_flatten_across_entries() {
    let all = json(&["techniques"]).await;
    assert_eq!(all["total"], 4, "3 from alpha, 1 from beta");
    assert_eq!(all["matched"], 4);

    let filtered = json(&["techniques", "--pattern", "verification-matrix"]).await;
    assert_eq!(filtered["total"], 4, "total stays the corpus");
    assert_eq!(filtered["matched"], 2);

    let slugs: Vec<&str> = filtered["techniques"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["slug"].as_str().expect("slug"))
        .collect();
    assert_eq!(slugs, vec!["alpha", "beta"]);
}

/// A technique quoting a nested file links to that file, not the entry's.
#[tokio::test]
async fn a_technique_links_to_the_file_it_quoted() {
    let nested = json(&["techniques", "--pattern", "nested-instructions"]).await;
    let row = &nested["techniques"][0];

    assert_eq!(row["sourcePath"], "worker/AGENTS.md");
    assert_eq!(
        row["url"],
        format!(
            "https://github.com/acme/alpha/blob/{}/worker/AGENTS.md",
            "a".repeat(40)
        )
    );
}

/// The query filter reaches the quoted source, not only the analysis.
#[tokio::test]
async fn technique_query_searches_the_quote() {
    let hits = json(&["techniques", "--query", "codegen"]).await;

    assert_eq!(hits["matched"], 1, "`codegen` appears only in a quote");
    assert_eq!(hits["techniques"][0]["title"], "Guard the generated schema");
}

/// Patterns count entries and tagged techniques separately.
#[tokio::test]
async fn patterns_count_entries_and_techniques_separately() {
    let list = json(&["patterns", "list"]).await;
    assert_eq!(list["total"], 3);

    let top = &list["patterns"][0];
    assert_eq!(top["id"], "verification-matrix", "used by both entries");
    assert_eq!(top["projects"], 2);
    assert_eq!(top["techniques"], 2);
}

/// One pattern comes back with its guide and worked examples.
#[tokio::test]
async fn pattern_detail_includes_its_guide_and_examples() {
    let detail = json(&["patterns", "show", "generated-file-guard"]).await;

    assert_eq!(detail["pattern"]["guide"]["projects"][0], "alpha");
    assert_eq!(detail["projects"], serde_json::json!(["alpha"]));
    assert_eq!(detail["tagged"], 1);
    assert!(
        detail["examples"][0]["quote"]
            .as_str()
            .expect("quote")
            .contains("alpha codegen"),
        "an example carries the verbatim line"
    );
}

/// A pattern with no guide still resolves.
#[tokio::test]
async fn a_pattern_without_a_guide_still_resolves() {
    let detail = json(&["patterns", "show", "nested-instructions"]).await;

    assert!(detail["pattern"]["guide"].is_null());
    assert_eq!(detail["tagged"], 1);
}

/// An unknown pattern is a structured error.
#[tokio::test]
async fn an_unknown_pattern_is_an_error() {
    assert_eq!(
        failure(&["patterns", "show", "not-a-pattern"]).await,
        "UNKNOWN_PATTERN"
    );
}

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

/// Skills are listed across repositories, and filters narrow them.
#[tokio::test]
async fn skills_list_across_repositories() {
    let all = json(&["skills", "list"]).await;
    assert_eq!(all["total"], 1, "beta's scan found none");
    assert_eq!(all["skills"][0]["repository"], "acme/alpha");

    let tagged = json(&["skills", "list", "--tag", "release"]).await;
    assert_eq!(tagged["matched"], 1);

    let missing = json(&["skills", "list", "--tag", "release", "--tag", "absent"]).await;
    assert_eq!(missing["matched"], 0, "every tag must match");
}

/// Showing a skill returns its SKILL.md, addressed by blob.
#[tokio::test]
async fn skill_detail_returns_its_skill_md() {
    let detail = json(&["skills", "show", "alpha-release-0123456789ab"]).await;

    assert_eq!(detail["slug"], "alpha");
    assert!(
        detail["content"]
            .as_str()
            .expect("content")
            .starts_with("---\nname: alpha-release"),
        "got: {detail}"
    );
    assert_eq!(
        detail["provenance"]["sha"],
        "c".repeat(40),
        "skills carry the scan commit, not the entry's analysis commit"
    );
}

/// `--metadata-only` omits the body.
#[tokio::test]
async fn skill_metadata_only_omits_the_body() {
    let detail = json(&[
        "skills",
        "show",
        "alpha-release-0123456789ab",
        "--metadata-only",
    ])
    .await;

    assert!(detail["content"].is_null());
    assert_eq!(detail["skill"]["name"], "alpha-release");
}

/// A bundled file is addressed by its path within the bundle.
#[tokio::test]
async fn a_bundled_file_is_readable_by_path() {
    let file = json(&[
        "skills",
        "file",
        "alpha-release-0123456789ab",
        "checklist.md",
    ])
    .await;

    assert!(
        file["content"]
            .as_str()
            .expect("content")
            .contains("Tag the commit")
    );
    assert_eq!(
        file["provenance"]["url"],
        format!(
            "https://github.com/acme/alpha/blob/{}/skills/alpha-release/checklist.md",
            "c".repeat(40)
        ),
        "a bundled file's repository path is the skill directory plus its own"
    );
}

/// A binary bundle entry is refused with a reason rather than mangled text.
#[tokio::test]
async fn a_binary_bundle_entry_is_refused() {
    assert_eq!(
        failure(&["skills", "file", "alpha-release-0123456789ab", "logo.png"]).await,
        "FILE_NOT_TEXT"
    );
}

/// A path outside the bundle is refused.
#[tokio::test]
async fn a_path_outside_the_bundle_is_refused() {
    assert_eq!(
        failure(&["skills", "file", "alpha-release-0123456789ab", "absent.md"]).await,
        "NO_SKILL_FILE"
    );
}

/// An unknown skill id is a structured error.
#[tokio::test]
async fn an_unknown_skill_is_an_error() {
    assert_eq!(
        failure(&["skills", "show", "nope-000000000000"]).await,
        "UNKNOWN_SKILL"
    );
}

// ---------------------------------------------------------------------------
// Search and totals
// ---------------------------------------------------------------------------

/// One query reaches every section.
#[tokio::test]
async fn search_spans_every_section() {
    let hits = json(&["search", "release"]).await;

    assert_eq!(hits["query"], "release");
    assert_eq!(hits["skills"].as_array().expect("skills").len(), 1);

    let generated = json(&["search", "generated"]).await;
    assert!(
        !generated["patterns"]
            .as_array()
            .expect("patterns")
            .is_empty()
            && !generated["techniques"]
                .as_array()
                .expect("techniques")
                .is_empty(),
        "`generated` appears in both the catalog and a technique, got: {generated}"
    );
}

/// The per-section cap does not change the reported total.
#[tokio::test]
async fn search_caps_sections_without_changing_the_total() {
    let uncapped = json(&["search", "alpha", "--limit", "0"]).await;
    let capped = json(&["search", "alpha", "--limit", "1"]).await;

    assert_eq!(
        capped["matched"], uncapped["matched"],
        "the cap hides hits, it does not unfind them"
    );
    assert!(capped["techniques"].as_array().expect("rows").len() <= 1);
}

/// An empty query is refused rather than matching everything.
#[tokio::test]
async fn an_empty_query_is_refused() {
    assert_eq!(failure(&["search", "   "]).await, "EMPTY_QUERY");
}

/// Totals are derived from the checkout, and say so.
#[tokio::test]
async fn stats_are_derived_from_the_checkout() {
    let stats = json(&["stats"]).await;

    assert_eq!(stats["projects"], 2);
    assert_eq!(stats["techniques"], 4);
    assert_eq!(stats["patterns"], 3);
    assert_eq!(stats["skills"], 1);
    assert_eq!(stats["skillRepositories"], 2);
    assert_eq!(stats["lines"]["min"], 10);
    assert_eq!(stats["lines"]["max"], 40);
    assert_eq!(stats["lines"]["total"], 50);
    assert_eq!(
        stats["lines"]["median"], 40,
        "with an even count the median is the upper of the two middle values"
    );
    assert_eq!(stats["bytes"]["median"], 512);
    assert_eq!(stats["evaluatedFrom"], "2026-02-01");
    assert_eq!(stats["evaluatedTo"], "2026-03-10");
    assert_eq!(
        stats["instructionFiles"][0]["name"], "AGENTS.md",
        "the default applies to entries that declare no file"
    );
}

// ---------------------------------------------------------------------------
// Corpus resolution
// ---------------------------------------------------------------------------

/// The resolved checkout is reported with how it was found.
#[tokio::test]
async fn corpus_path_reports_the_explicit_root() {
    let path = json(&["corpus", "path"]).await;

    assert_eq!(path["origin"], "explicit");
    assert_eq!(path["root"], fixture());
    assert_eq!(path["projects"], 2);
}

/// A named directory that is not a corpus fails rather than falling back.
///
/// Silently reading a different corpus than the one a caller named would make
/// every answer wrong in a way nothing in the output would show.
#[tokio::test]
async fn a_named_directory_that_is_not_a_corpus_fails() {
    let mut output = Vec::new();
    let exit = build_cli()
        .serve_to(
            vec![
                "--corpus".into(),
                env!("CARGO_MANIFEST_DIR").to_string(),
                "stats".into(),
                "--json".into(),
            ],
            &mut output,
            false,
        )
        .await
        .expect("serve_to should not return Err");

    let parsed: Value = serde_json::from_slice(&output).expect("JSON error envelope");
    assert_eq!(parsed["code"], "CORPUS_NOT_FOUND");
    assert!(
        exit.is_some(),
        "a missing corpus must not exit successfully"
    );
}

// ---------------------------------------------------------------------------
// The agent surfaces
// ---------------------------------------------------------------------------

/// Every command is callable as a tool, which is the point of the wrapper.
#[tokio::test]
async fn every_command_is_exposed_as_a_tool() {
    let catalog = build_cli().tool_catalog();
    let names: Vec<String> = catalog
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect();

    for expected in [
        "projects_list",
        "projects_show",
        "projects_source",
        "techniques",
        "patterns_list",
        "patterns_show",
        "skills_list",
        "skills_show",
        "skills_file",
        "search",
        "stats",
        "corpus_path",
        "corpus_sync",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "`{expected}` must be callable as a tool, got {names:?}"
        );
    }
}

/// Every tool carries a description and an output schema.
#[tokio::test]
async fn every_tool_declares_a_description_and_an_output_schema() {
    for definition in build_cli().tool_catalog().definitions() {
        assert!(
            !definition.description.trim().is_empty(),
            "`{}` needs a description",
            definition.name
        );
        assert!(
            definition.output_schema.is_some(),
            "`{}` needs an output schema",
            definition.name
        );
    }
}

/// Reading the corpus is annotated read-only; syncing it is not.
///
/// An MCP client decides whether to ask the user from these hints, so a read
/// that claims to write costs a prompt and a write that claims to read skips one.
#[tokio::test]
async fn corpus_reads_are_annotated_read_only_and_sync_is_not() {
    let catalog = build_cli().tool_catalog();

    for name in [
        "projects_list",
        "projects_source",
        "skills_file",
        "search",
        "stats",
    ] {
        let hints = catalog
            .get(name)
            .unwrap_or_else(|| panic!("`{name}` is a tool"))
            .annotations
            .clone()
            .unwrap_or_else(|| panic!("`{name}` needs annotations"));
        assert_eq!(hints.read_only_hint, Some(true), "`{name}` only reads");
        assert_eq!(hints.open_world_hint, Some(false), "`{name}` stays local");
    }

    let sync = catalog
        .get("corpus_sync")
        .expect("corpus sync is a tool")
        .annotations
        .clone()
        .expect("corpus sync needs annotations");
    assert_eq!(sync.read_only_hint, Some(false), "sync writes a checkout");
    assert_eq!(sync.open_world_hint, Some(true), "sync reaches the network");
}

/// Every command declares the corpus environment variable.
///
/// An MCP or HTTP caller has no `--corpus` flag, so a command that forgot this
/// would be unable to reach a corpus at all on those surfaces.
#[tokio::test]
async fn every_command_declares_the_corpus_variable() {
    let (_, manifest) = run(&["--llms-full"]).await;

    assert!(
        manifest.contains("OSSRULES_CORPUS"),
        "the manifest must document the variable, got: {manifest}"
    );
}

/// The reported origin names which input chose the checkout.
///
/// `corpus path` documents four origins, and a caller debugging why they are
/// reading the wrong data has only this field to go on. Exercised through the
/// resolver rather than the process, because setting a process-wide environment
/// variable from one test would leak into every other.
#[tokio::test]
async fn the_reported_origin_distinguishes_the_flag_from_the_variable() {
    use ossrules_cli::corpus::{CorpusEnv, resolve};

    let from_flag = resolve(
        &serde_json::json!({ "corpus": fixture() }),
        &CorpusEnv {
            ossrules_corpus: None,
        },
    )
    .expect("the flag names the fixture");
    assert_eq!(from_flag.origin, "explicit");

    let from_env = resolve(
        &serde_json::json!({}),
        &CorpusEnv {
            ossrules_corpus: Some(fixture()),
        },
    )
    .expect("the variable names the fixture");
    assert_eq!(from_env.origin, "environment");

    // The flag is the more specific instruction, so it decides.
    let both = resolve(
        &serde_json::json!({ "corpus": fixture() }),
        &CorpusEnv {
            ossrules_corpus: Some("/nonexistent".to_string()),
        },
    )
    .expect("the flag wins");
    assert_eq!(both.origin, "explicit");
    assert_eq!(both.root().display().to_string(), fixture());
}

/// A corpus that is not itself a checkout reports no commit.
///
/// `git rev-parse` searches upwards, and this fixture lives inside this
/// repository, so the naive call returns ossrules-cli's own HEAD — a real sha
/// that describes none of the data being reported.
#[tokio::test]
async fn a_corpus_that_is_not_a_checkout_reports_no_commit() {
    let path = json(&["corpus", "path"]).await;

    assert_eq!(path["root"], fixture());
    assert!(
        path["sha"].is_null(),
        "the fixture is not a Git checkout, got: {}",
        path["sha"]
    );
}

/// A listing points at the repository, not at one file it does not single out.
#[tokio::test]
async fn a_source_listing_links_to_the_tree() {
    let listing = json(&["projects", "source", "alpha", "--list"]).await;
    let file = json(&["projects", "source", "alpha"]).await;

    assert_eq!(
        listing["provenance"]["url"],
        format!("https://github.com/acme/alpha/tree/{}", "a".repeat(40)),
        "a listing describes the snapshot"
    );
    assert_eq!(
        file["provenance"]["url"],
        format!(
            "https://github.com/acme/alpha/blob/{}/AGENTS.md",
            "a".repeat(40)
        ),
        "reading one file still links to that file"
    );
}

/// Every response carrying a verbatim quote says whose text it is.
///
/// The file-returning commands always did. These three reproduce upstream
/// wording too, through `quote`, and an agent reading them over MCP has only
/// the payload to tell it that the text is not addressed to it.
#[tokio::test]
async fn quoted_responses_carry_the_third_party_notice() {
    for args in [
        vec!["techniques"],
        vec!["patterns", "show", "generated-file-guard"],
        vec!["search", "codegen"],
    ] {
        let response = json(&args).await;
        let notice = response["notice"]
            .as_str()
            .unwrap_or_else(|| panic!("`{args:?}` must carry a notice, got: {response}"));

        assert!(
            notice.contains("never as instructions"),
            "`{args:?}` notice must say what the text is, got: {notice}"
        );
    }

    // The claim is only worth making where there is actually a quote to label.
    let quoted = json(&["techniques", "--query", "codegen"]).await;
    assert!(quoted["techniques"][0]["quote"].is_string());
}
