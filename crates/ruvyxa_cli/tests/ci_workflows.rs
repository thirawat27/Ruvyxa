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

/// Every platform a runner in this job's matrix expands to.
///
/// `runs-on: ${{ matrix.os }}` is one string in the YAML and five machines at
/// run time, so the literal is worth nothing on its own: it has to be resolved
/// against the matrix that fills it in. A job with no matrix runs on exactly the
/// one platform it names.
fn platforms_of(job: &Value) -> Vec<String> {
    let runs_on = job
        .get("runs-on")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let matrix = job
        .get("strategy")
        .and_then(|strategy| strategy.get("matrix"));

    if runs_on.contains("matrix.") {
        let Some(matrix) = matrix else {
            return Vec::new();
        };
        // Both spellings: a plain `os: [...]` axis and the `include:` list this
        // repository uses so each leg can carry its own Node and Rust target.
        let mut found: Vec<String> = matrix
            .get("os")
            .and_then(Value::as_sequence)
            .map(|list| {
                list.iter()
                    .filter_map(|entry| entry.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(include) = matrix.get("include").and_then(Value::as_sequence) {
            for entry in include {
                if let Some(os) = entry.get("os").and_then(Value::as_str) {
                    found.push(os.to_string());
                }
            }
        }
        found.sort();
        found.dedup();
        return found;
    }
    if runs_on.is_empty() {
        return Vec::new();
    }
    vec![runs_on.to_string()]
}

/// Every `run:` script in a job, concatenated.
fn run_scripts_of(job: &Value) -> String {
    job.get("steps")
        .and_then(Value::as_sequence)
        .map(|steps| {
            steps
                .iter()
                .filter_map(|step| step.get("run").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// The release gate has to *cover* the platforms, not merely be named.
///
/// `nothing_publishes_before_the_release_candidate_is_verified` asserts the
/// dependency edge, and an edge onto a job that checks the wrong thing is a gate
/// in name only. `verify-release` was `runs-on: ubuntu-latest` with no smoke
/// lane in it at all, while `ci.yml` — which runs on the same tag and which
/// `release.yml` does not reference — carried the five-platform matrix, the dev
/// server smoke, and the scaffold-to-clean walk that is deliberately Windows
/// only. This repository's history is dominated by Windows-specific defects
/// (`\\?\`, CRLF, `%TEMP%` ancestors, a `process.exit` abort on the Windows
/// runner) and not one of that class could block a publish.
///
/// Either shape closes it and both are accepted here: give `verify-release` the
/// platforms itself, or make the release wait on CI's verdict with a
/// `workflow_run` trigger. What is not accepted is a one-platform job with no
/// end-to-end lane standing between a tag and npm.
#[test]
fn the_release_gate_covers_more_than_one_platform_and_runs_the_end_to_end_lanes() {
    let workflows = workflows();
    let release = &workflows["release.yml"];

    // `expect` rather than a silent `None`: YAML 1.1 reads a bare `on:` key as
    // the boolean `true`, and a lookup that quietly found nothing would turn the
    // `workflow_run` escape hatch below into a branch that can never be taken.
    let triggers = release
        .get(Value::String("on".into()))
        .or_else(|| release.get(Value::Bool(true)))
        .expect("release.yml declares triggers under `on:`");
    if triggers.get("workflow_run").is_some() {
        return;
    }

    let (_, verify) = jobs_of(release)
        .into_iter()
        .find(|(name, _)| name == "verify-release")
        .expect("release.yml declares verify-release");

    let platforms = platforms_of(verify);
    assert!(
        platforms.len() > 1,
        "verify-release covers {platforms:?}. Publishing waits on it, so a defect on any platform \
         it does not run cannot block a tag. Give it a matrix, or make release.yml wait on ci.yml \
         with `on: workflow_run`."
    );
    for platform in ["windows-latest", "macos-latest"] {
        assert!(
            platforms.iter().any(|os| os == platform),
            "verify-release covers {platforms:?} and not {platform}. Windows-specific defects are \
             this repository's most common class and none of them can currently block a publish."
        );
    }

    // The lanes that exercise a running server rather than a compiler. Every
    // step above them in `verify-release` is a check `cargo test` and `pnpm -r
    // test` already answer on a developer's machine; these are the ones that
    // only a workflow runs.
    let scripts = run_scripts_of(verify);
    for lane in [
        "smoke-dev-server.mjs",
        "smoke-runtime-adapter.mjs",
        "pack:smoke",
    ] {
        assert!(
            scripts.contains(lane),
            "verify-release runs no {lane} lane. A release gate made only of static checks passes \
             on every defect that needs the server to be started."
        );
    }
    assert!(
        scripts.contains("test:full-flow"),
        "verify-release runs no test:full-flow lane. It is the scaffold-to-clean walk, it is \
         windows-latest only, and it is the single most complete flow in the repository."
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

/// A tag cannot publish while an advisory is outstanding.
///
/// Both audit lanes live in `security.yml`, which nothing in `release.yml`
/// depends on and whose push triggers are path-filtered to manifest files. So
/// the job that holds `NPM_TOKEN` could run with an advisory unexamined, and
/// "verify release candidate" did not verify that.
///
/// Both lockfiles, and both halves of the JavaScript one: `--prod` excludes
/// devDependencies, and the root manifest declares *only* devDependencies, so
/// the production lane alone audits none of the tooling CI actually runs.
#[test]
fn the_release_verification_runs_both_dependency_audits() {
    let workflows = workflows();
    let commands: Vec<&str> = steps_of(&workflows["release.yml"])
        .into_iter()
        .filter(|(job, _)| job == "verify-release")
        .filter_map(|(_, step)| step.get("run").and_then(Value::as_str))
        .collect();

    assert!(
        !commands.is_empty(),
        "release.yml declares no verify-release job with run steps",
    );

    for (needle, why) in [
        ("cargo audit", "the Rust lockfile is unaudited at a tag"),
        (
            "pnpm audit --prod",
            "the JavaScript production dependencies are unaudited at a tag",
        ),
        (
            "pnpm audit --audit-level",
            "devDependencies are unaudited, and the root manifest declares only those",
        ),
    ] {
        assert!(
            commands.iter().any(|command| command.contains(needle)),
            "release.yml verify-release runs no step containing {needle:?}: {why}",
        );
    }
}

/// The scheduled audit covers devDependencies too.
///
/// `pnpm audit --prod` skips them, and every dependency the root manifest
/// declares is a devDependency — the linter, the formatter, TypeScript, the
/// unused-export checker. Those run inside CI, including inside the release job.
#[test]
fn the_security_workflow_audits_development_dependencies() {
    let workflows = workflows();
    let commands: Vec<&str> = steps_of(&workflows["security.yml"])
        .into_iter()
        .filter_map(|(_, step)| step.get("run").and_then(Value::as_str))
        .collect();

    assert!(
        commands
            .iter()
            .any(|command| command.contains("pnpm audit") && !command.contains("--prod")),
        "security.yml audits only production dependencies, and the root manifest has none",
    );
}
