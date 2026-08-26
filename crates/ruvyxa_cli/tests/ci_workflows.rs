//! Properties of `.github/workflows/*.yml` that nothing else can hold.
//!
//! A workflow only runs on GitHub, so this repository cannot execute one to
//! find out whether it is still correct. What it can do is parse it. Every
//! assertion here exists because the property it names was broken at least once
//! and no gate noticed: a release published five native packages and then
//! failed, a commit on `main` was cancelled and shipped inside a tag with no
//! verdict, and the whole supply chain hung off mutable action tags.
//!
//! Matching the text would pass on precisely those bugs, so this parses.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above this crate")
}

fn workflows() -> BTreeMap<String, Value> {
    let dir = repo_root().join(".github/workflows");
    let mut parsed = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).expect("the workflow directory exists") {
        let path = entry.expect("a readable directory entry").path();
        if path.extension().and_then(|value| value.to_str()) != Some("yml") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .expect("a UTF-8 workflow name")
            .to_string();
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let document: Value = serde_yaml_ng::from_str(&source)
            .unwrap_or_else(|error| panic!("{name} is not valid YAML: {error}"));
        parsed.insert(name, document);
    }
    assert!(!parsed.is_empty(), "no workflows were found to check");
    parsed
}

/// Every job in a workflow, by name.
fn jobs_of(document: &Value) -> Vec<(String, &Value)> {
    let Some(jobs) = document.get("jobs").and_then(Value::as_mapping) else {
        return Vec::new();
    };
    jobs.iter()
        .map(|(name, job)| (name.as_str().unwrap_or("<unnamed>").to_string(), job))
        .collect()
}

/// Every step in a workflow, paired with the job it belongs to.
fn steps_of(document: &Value) -> Vec<(String, &Value)> {
    let mut steps = Vec::new();
    for (name, job) in jobs_of(document) {
        if let Some(list) = job.get("steps").and_then(Value::as_sequence) {
            for step in list {
                steps.push((name.clone(), step));
            }
        }
    }
    steps
}

fn is_commit_sha(reference: &str) -> bool {
    reference.len() == 40
        && reference
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

/// A tag is a moving target, and this repository publishes 24 npm packages.
///
/// `actions/checkout@v7` resolves to whatever that tag points at on the day the
/// job runs. An action that is retagged — or whose account is taken over — runs
/// inside the release workflow, next to `NPM_TOKEN`. Pinning the commit makes
/// third-party code an input somebody has to change on purpose, and the
/// trailing `# v7` keeps it readable.
#[test]
fn every_action_is_pinned_to_a_commit() {
    for (file, document) in workflows() {
        for (job, step) in steps_of(&document) {
            let Some(uses) = step.get("uses").and_then(Value::as_str) else {
                continue;
            };
            let (action, reference) = uses
                .rsplit_once('@')
                .unwrap_or_else(|| panic!("{file} · {job}: `uses: {uses}` names no ref"));
            assert!(
                is_commit_sha(reference),
                "{file} · {job}: {action} is pinned to {reference:?}, which can move. Pin the commit and leave the readable ref in a trailing comment."
            );
        }
    }
}

/// Checkout writes a usable credential into `.git/config` unless told not to.
///
/// Every job here only reads the repository; none of them pushes. Anything
/// running afterwards — a build script, a transitive dependency — can reach
/// that credential, so it should never have been written.
#[test]
fn no_checkout_keeps_the_repository_credential() {
    for (file, document) in workflows() {
        for (job, step) in steps_of(&document) {
            let Some(uses) = step.get("uses").and_then(Value::as_str) else {
                continue;
            };
            if !uses.starts_with("actions/checkout@") {
                continue;
            }
            let persisted = step
                .get("with")
                .and_then(|with| with.get("persist-credentials"))
                .and_then(Value::as_bool);
            assert_eq!(
                persisted,
                Some(false),
                "{file} · {job}: checkout must set `persist-credentials: false`"
            );
        }
    }
}

/// The JavaScript publish job runs on whatever Rust the runner image ships.
///
/// It is the one cargo-reachable job in the release workflow with no toolchain
/// step, and an unfiltered `pnpm -r build` walks into `examples/demo`, whose
/// build script is `cargo run -p ruvyxa_cli`. The day the workspace MSRV moved
/// to 1.98 that became a release which published five native packages and then
/// failed. Nothing in CI can catch it: every job there installs Rust first, and
/// this one only runs on a tag.
///
/// Either keep cargo out of this job, or give it a toolchain. Silently
/// inheriting the image's rustc is the arrangement that broke.
#[test]
fn the_javascript_publish_job_never_reaches_cargo() {
    let release = workflows();
    let job = release["release.yml"]
        .get("jobs")
        .and_then(|jobs| jobs.get("publish-packages"))
        .expect("release.yml declares a publish-packages job");
    let steps = job
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("publish-packages has steps");

    let installs_rust = steps.iter().any(|step| {
        step.get("uses")
            .and_then(Value::as_str)
            .is_some_and(|uses| uses.starts_with("dtolnay/rust-toolchain@"))
    });

    for step in steps {
        let Some(run) = step.get("run").and_then(Value::as_str) else {
            continue;
        };
        let name = step
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("<unnamed>");
        // `cargo` invoked directly, or a recursive build wide enough to reach a
        // workspace project whose own script invokes it.
        let reaches_cargo = run.split_whitespace().any(|word| word == "cargo")
            || (run.contains("pnpm -r build") && !run.contains("--filter"));
        assert!(
            !reaches_cargo || installs_rust,
            "release.yml · publish-packages · {name:?} can reach cargo while the job installs no Rust toolchain. Filter the build to ./packages/**, or add a toolchain step."
        );
    }
}

/// Nothing in the release workflow creates a release or pushes a commit.
///
/// `contents: write` at the workflow level handed a write-capable token to the
/// two jobs that only check out and build. The publish jobs ask for the
/// `id-token` they actually need themselves.
#[test]
fn the_release_workflow_asks_for_no_write_access_to_the_repository() {
    let workflows = workflows();
    let release = &workflows["release.yml"];
    let mut scopes = vec![release.get("permissions")];
    for (_, job) in jobs_of(release) {
        scopes.push(job.get("permissions"));
    }
    for permissions in scopes.into_iter().flatten() {
        assert_ne!(
            permissions.get("contents").and_then(Value::as_str),
            Some("write"),
            "release.yml grants contents: write, and no step uses it"
        );
    }
}

/// A job with no ceiling holds a runner until GitHub's six-hour default.
#[test]
fn every_job_declares_a_timeout() {
    for (file, document) in workflows() {
        let jobs = jobs_of(&document);
        assert!(!jobs.is_empty(), "{file} declares no jobs");
        for (name, job) in jobs {
            assert!(
                job.get("timeout-minutes").is_some(),
                "{file} · {name} has no timeout-minutes"
            );
        }
    }
}

/// Publishing waits on verification, and the JavaScript packages wait on the
/// binaries they are installed beside.
#[test]
fn nothing_publishes_before_the_release_candidate_is_verified() {
    let workflows = workflows();
    let release = &workflows["release.yml"];

    let needs = |wanted: &str| -> Vec<String> {
        let (_, job) = jobs_of(release)
            .into_iter()
            .find(|(name, _)| name == wanted)
            .unwrap_or_else(|| panic!("release.yml declares {wanted}"));
        match job
            .get("needs")
            .unwrap_or_else(|| panic!("{wanted} declares no needs"))
        {
            Value::String(one) => vec![one.clone()],
            Value::Sequence(many) => many
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect(),
            other => panic!("{wanted} has an unreadable `needs`: {other:?}"),
        }
    };

    for job in ["publish-native", "publish-packages"] {
        assert!(
            needs(job).contains(&"verify-release".to_string()),
            "{job} must wait for verify-release"
        );
    }
    assert!(
        needs("publish-packages").contains(&"publish-native".to_string()),
        "publish-packages must wait for the native binaries it is installed beside"
    );
}

/// A cancelled run leaves a commit with no verdict.
///
/// On a branch that is the point — the next push supersedes it. On `main` and
/// on a release tag it means something can ship having never gone green, which
/// is not hypothetical: the run on 307aece was cancelled by the next push, and
/// that commit was inside the tag that published.
#[test]
fn ci_never_cancels_a_run_on_main_or_a_tag() {
    let workflows = workflows();
    let cancel = workflows["ci.yml"]
        .get("concurrency")
        .and_then(|concurrency| concurrency.get("cancel-in-progress"))
        .expect("ci.yml declares cancel-in-progress");

    // An unconditional `true` is the shape that lost a verdict, so the value has
    // to be an expression that reads the ref.
    let expression = cancel
        .as_str()
        .unwrap_or_else(|| panic!("cancel-in-progress must be an expression, found {cancel:?}"));
    assert!(
        expression.contains("refs/heads/main") && expression.contains("refs/tags/"),
        "cancel-in-progress must exempt main and tags, found {expression:?}"
    );
}

/// CodeQL analyzes both halves of the framework, not whichever one is cheaper.
///
/// The compiler and server are Rust and the runtime and public API are
/// TypeScript, and a taint path that starts in one usually ends in the other.
/// Dropping a language from the matrix costs nothing visible — the workflow
/// still goes green, the security tab still has results, and the half nobody
/// analyzed is the half nobody notices.
///
/// `actions` is here for the same reason `ci_workflows.rs` exists: a workflow
/// is code, and it is the code that holds the credentials.
#[test]
fn codeql_analyzes_every_language_this_repository_is_written_in() {
    let workflows = workflows();
    let codeql = &workflows["codeql.yml"];
    let analyzed = codeql["jobs"]["analyze"]["strategy"]["matrix"]["include"]
        .as_sequence()
        .expect("codeql.yml declares a matrix of languages")
        .iter()
        .filter_map(|entry| entry.get("language").and_then(Value::as_str))
        .collect::<Vec<_>>();

    for language in ["rust", "javascript-typescript", "actions"] {
        assert!(
            analyzed.contains(&language),
            "codeql.yml must analyze {language}, found {analyzed:?}"
        );
    }
}

/// No job may acquire a grant that can change the repository.
///
/// Two writes are legitimate and both are narrow: `security-events` on the
/// CodeQL job, which is the only way an analysis reaches the security tab, and
/// `id-token` on the publish jobs, which mints a short-lived OIDC token for npm
/// provenance and can alter nothing here. Anything else — `contents`,
/// `pull-requests`, `packages` — would let a workflow added to *inspect* the
/// repository modify it, which is the shape a compromised action needs.
#[test]
fn no_job_may_write_anything_that_changes_the_repository() {
    for (file, document) in workflows() {
        for (name, job) in jobs_of(&document) {
            let Some(permissions) = job.get("permissions").and_then(Value::as_mapping) else {
                continue;
            };
            for (key, value) in permissions {
                let (Some(key), Some(value)) = (key.as_str(), value.as_str()) else {
                    continue;
                };
                if value != "write" {
                    continue;
                }
                let allowed = matches!(
                    (file.as_str(), key),
                    ("codeql.yml", "security-events") | (_, "id-token")
                );
                assert!(allowed, "{file} · {name} asks for {key}: write");
            }
        }
    }
}

/// `cargo test` runs before `pnpm -r build`, so no Rust test may need `dist/`.
///
/// The workspace packages resolve through their `exports`, which point at
/// `dist/`, and `require.resolve` fails when that file is absent. A Rust test
/// that resolves one therefore passes on any developer machine — where a
/// previous build left `dist/` behind — and fails on a cold checkout, which is
/// the only place it matters. `an_adapter_resolves_before_a_build_has_produced
/// _anything` was written against `@ruvyxa/adapter-node` and did exactly that;
/// it now fabricates its own adapter package inside the temp project.
///
/// The ordering is what makes that a rule rather than a preference. Building
/// packages first would loosen it, and this gate is here so that becomes a
/// decision instead of an accident that quietly re-enables the trap.
#[test]
fn no_workflow_builds_packages_before_it_tests_rust() {
    for (file, document) in workflows() {
        for (name, job) in jobs_of(&document) {
            let Some(steps) = job.get("steps").and_then(Value::as_sequence) else {
                continue;
            };
            let position_of = |needle: &str| {
                steps.iter().position(|step| {
                    step.get("run")
                        .and_then(Value::as_str)
                        .is_some_and(|run| run.contains(needle))
                })
            };
            let (Some(tests_rust), Some(builds_packages)) =
                (position_of("cargo test"), position_of("pnpm -r build"))
            else {
                continue;
            };
            assert!(
                tests_rust < builds_packages,
                "{file} · {name} builds packages at step {builds_packages} and tests Rust at \
                 step {tests_rust}. Rust tests here are written to need no `dist/`; if that is \
                 no longer wanted, change them and delete this gate on purpose."
            );
        }
    }
}
