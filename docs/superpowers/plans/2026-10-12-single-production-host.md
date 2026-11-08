# Single Production Host Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `ruvyxa start` serve every production request through the standalone JavaScript
server (`createHandler`), with `--workers`, a Rust image sidecar for `image.onDemand`, and a
one-release `--legacy-host` rollback — after a green baseline and an experimental tier for the four
integration packages.

**Architecture:** `ruvyxa build` always leaves a standalone server for the configured runtime at
`<outDir>/deploy/<runtime>/` by running the built-in `node`/`bun`/`deno` adapter (the same code a
self-hosted deployment runs), stamped with the build id. `ruvyxa start` becomes a Rust process
supervisor: it checks the stamp, optionally serves the existing Rust image pipeline on a loopback
port, spawns the standalone server, forwards signals, and exits with its code. The standalone server
learns cluster supervision and an `optimizeImage` that forwards to the sidecar.

**Tech Stack:** Rust (tokio, axum, clap, libc on unix), TypeScript in `@ruvyxa/core` emitting
generated JavaScript, Node test runner, the existing smoke harness.

**Spec:**
[`2026-10-12-single-production-host-design.md`](../specs/2026-10-12-single-production-host-design.md)

## Global Constraints

- **Never run `git commit`, `git add`, or `git push`.** Every task ends at a checkpoint: show the
  diff (`git diff HEAD --stat`) and report what ran. The owner commits.
- Preserve changes you did not make: check `git diff HEAD` before editing a file; never
  `git checkout --` or `git restore` anything.
- No pinned version numbers in `docs/`; no other framework named in `docs/`.
- `localeCompare` and `toLocale*Case` are banned (oxlint); sort with `compareCodePoints` from
  `runtime/order.mjs` or code-point comparison.
- Generated server source is a template string: a backtick in a comment inside it breaks the emitted
  file. Every change to emitted source is covered by the `node --check` test in
  `tests/packages/core/standalone-server.test.ts`.
- Each RUV code has exactly one meaning; the gate is `one_diagnostic_code_carries_one_meaning` in
  `crates/ruvyxa_diagnostics/src/lib.rs`. New codes: `RUV1022`, `RUV1023`, `RUV1024`, `RUV1025`
  (verified unused on 2026-10-12).
- Windows: anything used as a lookup key goes through `normalized_canonical_path`.
- If a JavaScript suite fails on a missing export, run `pnpm -r build` first — stale `dist/`.
- Battery (CLAUDE.md): `cargo fmt --all -- --check`,
  `cargo clippy --workspace --locked -- -D warnings`, `cargo test --workspace --locked`,
  `pnpm -r build`, `pnpm -r check`, `pnpm -r test`, `pnpm lint`, `pnpm format:check`,
  `pnpm check:unused`, `pnpm release:validate`, `pnpm pack:smoke`.

## File Map

| File                                                                          | Responsibility                                             |
| ----------------------------------------------------------------------------- | ---------------------------------------------------------- |
| `scripts/check-package-tiers.mjs` (new)                                       | Experimental-tier gate                                     |
| `tests/packages/ruvyxa/package-tiers.test.mjs` (new)                          | Gate unit tests                                            |
| `packages/@ruvyxa/{auth,database,realtime,testing}/package.json`, `README.md` | Tier field and warning                                     |
| `crates/ruvyxa_dev_server/src/framework_endpoints.rs`                         | `dynamic_image_response` extracted, shared by both callers |
| `crates/ruvyxa_dev_server/src/image_service.rs` (new)                         | Loopback image router + spawn helper                       |
| `packages/@ruvyxa/core/src/standalone-cluster.ts` (new)                       | Worker-count and crash-loop rules, supervisor source       |
| `packages/@ruvyxa/core/src/standalone-server.ts`                              | Wire supervisor, image-service optimizer                   |
| `crates/ruvyxa_cli/src/start.rs` (new)                                        | `start` launcher: checks, sidecar, spawn, signals          |
| `crates/ruvyxa_cli/src/build.rs`                                              | Local standalone stage + build-id stamp                    |
| `crates/ruvyxa_cli/src/runtime_config.rs`                                     | `manifest_build_id` extracted                              |
| `crates/ruvyxa_cli/src/main.rs`                                               | `StartArgs`, dispatch                                      |
| `scripts/smoke-runtime-adapter.mjs`, `.github/workflows/ci.yml`               | `start` lane                                               |
| `docs/en`, `docs/th`, `ARCHITECTURE.md`, `AGENTS.md`, `CHANGELOG.md`          | Documentation                                              |

---

### Task 0: Amend the spec to match two decisions taken while planning

**Files:**

- Modify: `docs/superpowers/specs/2026-10-12-single-production-host-design.md`

- [ ] **Step 1: Replace the standalone location.** In "Components" item 1, replace the
      `.ruvyxa/standalone/` description with:

```markdown
1. **Local standalone stage** (`crates/ruvyxa_cli/src/build.rs`). Today a build with no adapter
   skips the adapter stage entirely. After this change every build also runs the built-in adapter
   named after the configured `runtime` (`node`, `bun`, or `deno`) when the selected adapter is not
   already that one, so `<outDir>/deploy/<runtime>/` always holds the standalone server — the same
   directory and the same generator a self-hosted deployment uses. The stage writes
   `<outDir>/deploy/<runtime>/.ruvyxa-build-id` holding `manifest.json`'s `deploy.buildId`.
```

Replace every other `.ruvyxa/standalone/` in the file with `<outDir>/deploy/<runtime>/`.

- [ ] **Step 2: Add the image decision.** After the `--workers N` section add:

```markdown
### `image.onDemand` under `start`

The standalone server has no image pipeline, and a second one in JavaScript would be exactly the
duplication this programme removes. `ruvyxa start` serves the existing Rust pipeline
(`dynamic_image::optimize`) on `127.0.0.1:<ephemeral>` and passes `RUVYXA_IMAGE_SERVICE` and
`RUVYXA_IMAGE_SERVICE_TOKEN` to the child. The standalone server, when `image.onDemand` is on and
those are set, hands `createHandler` an `optimizeImage` that forwards to the sidecar — the shape the
Vercel and Cloudflare adapters already use for their platforms' optimizers. A deployment started
with `node server/index.mjs` directly has no sidecar and answers 404 exactly as it does today;
`tests/fixtures/adapter-contract.json` keeps `onDemandImages: false` for node/bun/deno because it
describes the deployed artifact, not `start`.
```

- [ ] **Step 3: Replace the fixtures paragraph** with:

```markdown
Phase 1 retires none: Axum still serves `dev` and `--legacy-host`. The tables that exist only to
hold Axum and `createHandler` together are listed below, so Phase 2 starts from a list. The fixtures
themselves are not edited — several of their readers assert an exact key set.
```

(Task 8 fills the list.)

- [ ] **Step 4: Add the codes table** under "Error handling for `start`":

```markdown
| Code      | Meaning                                                           |
| --------- | ----------------------------------------------------------------- |
| `RUV1022` | Standalone server missing or older than the build output          |
| `RUV1023` | The JavaScript runtime `start` needs was not found                |
| `RUV1024` | Realtime is configured, and realtime is served by `--legacy-host` |
| `RUV1025` | `RUVYXA_WORKERS` is invalid, or unsupported on this runtime/OS    |
```

- [ ] **Step 5: Verify** —
      `pnpm exec prettier --write docs/superpowers/specs/2026-10-12-single-production-host-design.md`
      then `node scripts/check-doc-links.mjs`. Expected: `Markdown links resolve across N files.`

- [ ] **Step 6: Checkpoint** — report the diff. Do not commit.

---

### Task 1: Phase 0 baseline

**Files:**

- Modify: `docs/superpowers/specs/2026-10-12-single-production-host-design.md` (append "Baseline")
- Modify: whatever the failures point at

- [ ] **Step 1: Record the tree state.** Run `git status --short` and `git diff HEAD --stat`. If
      either is non-empty, stop and ask the owner whether those changes are theirs before running
      anything that writes.

- [ ] **Step 2: Run the battery in order**, one command at a time, capturing the last 30 lines of
      each:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --locked -- -D warnings
cargo test --workspace --locked
pnpm -r build
pnpm -r check
pnpm -r test
pnpm lint
pnpm format:check
pnpm check:unused
pnpm release:validate
pnpm pack:smoke
```

- [ ] **Step 3: Append a "Baseline" section** to the spec: date, commit
      (`git rev-parse --short HEAD`), and one row per command — `pass`, or `fail` with the shortest
      decisive error line.

- [ ] **Step 4: For each failure**, use superpowers:systematic-debugging. Before blaming code, rule
      out the known false failures: stale `dist/` (rebuild), a loose `deno.json` in `%TEMP%`
      (create-ruvyxa PM test), host-RAM-dependent assertions (`prerender_parallelism`). A real
      defect gets a failing test first, then the root-cause fix. Update its Baseline row to
      `fixed: <one line>`.

- [ ] **Step 5: Re-run the full battery.** Expected: all eleven pass. Phase 1 does not start until
      they do.

- [ ] **Step 6: Checkpoint** — report the Baseline table and the diff. Do not commit.

---

### Task 2: Experimental tier for the four integration packages

**Files:**

- Create: `scripts/check-package-tiers.mjs`
- Create: `tests/packages/ruvyxa/package-tiers.test.mjs`
- Modify: `packages/@ruvyxa/{auth,database,realtime,testing}/package.json`
- Modify: `packages/@ruvyxa/{auth,database,realtime,testing}/README.md`
- Modify: `package.json` (`release:validate`)

**Interfaces:**

- Produces: `tierFailures(packages)` where `packages` is
  `Array<{ name: string, manifest: object, readme: string }>`, returning `string[]` (one message per
  failure, sorted by code point).
- Produces: constant `EXPERIMENTAL_WARNING = '> **Experimental.**'`.

- [ ] **Step 1: Write the failing test** `tests/packages/ruvyxa/package-tiers.test.mjs`:

```js
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { EXPERIMENTAL_WARNING, tierFailures } from '../../../scripts/check-package-tiers.mjs'

const experimental = (name, extra = {}) => ({
  name,
  manifest: { name, ruvyxa: { tier: 'experimental' }, ...extra },
  readme: `# ${name}\n\n${EXPERIMENTAL_WARNING} No production guarantee.\n`,
})
const stable = (name, dependencies = {}) => ({
  name,
  manifest: { name, dependencies },
  readme: `# ${name}\n`,
})

describe('package tiers', () => {
  it('accepts a marked experimental package and a stable one that does not depend on it', () => {
    assert.deepEqual(tierFailures([experimental('@ruvyxa/auth'), stable('ruvyxa')]), [])
  })

  it('requires the README warning on an experimental package', () => {
    const pkg = experimental('@ruvyxa/auth')
    pkg.readme = '# @ruvyxa/auth\n'
    assert.deepEqual(tierFailures([pkg]), [
      '@ruvyxa/auth is experimental but its README does not open with "> **Experimental.**"',
    ])
  })

  it('rejects an unknown tier value', () => {
    const pkg = experimental('@ruvyxa/auth')
    pkg.manifest.ruvyxa.tier = 'beta'
    assert.deepEqual(tierFailures([pkg]), [
      '@ruvyxa/auth declares ruvyxa.tier "beta"; the only tier a package may declare is "experimental"',
    ])
  })

  it('refuses a stable package depending on an experimental one, in any dependency field', () => {
    for (const field of ['dependencies', 'peerDependencies', 'optionalDependencies']) {
      const failures = tierFailures([
        experimental('@ruvyxa/realtime'),
        {
          ...stable('ruvyxa'),
          manifest: { name: 'ruvyxa', [field]: { '@ruvyxa/realtime': 'workspace:^' } },
        },
      ])
      assert.deepEqual(failures, [
        `ruvyxa is Tier 1 but its ${field} names experimental @ruvyxa/realtime`,
      ])
    }
  })

  it('requires the four integration packages to be marked', () => {
    assert.deepEqual(tierFailures([stable('@ruvyxa/database')]), [
      '@ruvyxa/database must declare "ruvyxa": { "tier": "experimental" }',
    ])
  })
})
```

- [ ] **Step 2: Run it to see it fail.** Run:
      `node --test tests/packages/ruvyxa/package-tiers.test.mjs` Expected: FAIL —
      `Cannot find module ... check-package-tiers.mjs`.

- [ ] **Step 3: Implement** `scripts/check-package-tiers.mjs`:

```js
#!/usr/bin/env node
/**
 * The experimental tier, enforced.
 *
 * `@ruvyxa/auth`, `@ruvyxa/database`, `@ruvyxa/realtime`, and `@ruvyxa/testing`
 * ship without a production guarantee. That is only honest while two things
 * hold: each says so where a reader looks first, and nothing in Tier 1 depends
 * on one — otherwise the guarantee a Tier 1 package makes silently includes code
 * that carries none.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

import { workspacePackageDirs } from './workspace-packages.mjs'

export const EXPERIMENTAL_WARNING = '> **Experimental.**'

const MUST_BE_EXPERIMENTAL = [
  '@ruvyxa/auth',
  '@ruvyxa/database',
  '@ruvyxa/realtime',
  '@ruvyxa/testing',
]
const DEPENDENCY_FIELDS = ['dependencies', 'peerDependencies', 'optionalDependencies']

const byCodePoint = (left, right) => (left < right ? -1 : left > right ? 1 : 0)

/** The first line after the `# title` and its blank line. */
function openingLine(readme) {
  return (
    readme
      .split(/\r?\n/)
      .slice(1)
      .find((line) => line.trim() !== '') ?? ''
  )
}

export function tierFailures(packages) {
  const failures = []
  const experimental = new Set()
  for (const { name, manifest, readme } of packages) {
    const tier = manifest.ruvyxa?.tier
    if (tier !== undefined && tier !== 'experimental') {
      failures.push(
        `${name} declares ruvyxa.tier "${tier}"; the only tier a package may declare is "experimental"`,
      )
      continue
    }
    if (tier === 'experimental') {
      experimental.add(name)
      if (!openingLine(readme).startsWith(EXPERIMENTAL_WARNING)) {
        failures.push(
          `${name} is experimental but its README does not open with "${EXPERIMENTAL_WARNING}"`,
        )
      }
    } else if (MUST_BE_EXPERIMENTAL.includes(name)) {
      failures.push(`${name} must declare "ruvyxa": { "tier": "experimental" }`)
    }
  }
  for (const { name, manifest } of packages) {
    if (experimental.has(name)) continue
    for (const field of DEPENDENCY_FIELDS) {
      for (const dependency of Object.keys(manifest[field] ?? {})) {
        if (experimental.has(dependency)) {
          failures.push(`${name} is Tier 1 but its ${field} names experimental ${dependency}`)
        }
      }
    }
  }
  return failures.sort(byCodePoint)
}

function main() {
  const { dirs } = workspacePackageDirs()
  const packages = dirs.map((dir) => {
    const manifest = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'))
    let readme = ''
    try {
      readme = readFileSync(join(dir, 'README.md'), 'utf8')
    } catch {
      // A package with no README cannot carry the warning; tierFailures says so.
    }
    return { name: manifest.name, manifest, readme }
  })
  const failures = tierFailures(packages)
  for (const failure of failures) console.error(`check-package-tiers: ${failure}`)
  if (failures.length > 0) process.exit(1)
  console.log(`Package tiers hold across ${packages.length} packages.`)
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) main()
```

Before relying on `workspacePackageDirs()`, open `scripts/workspace-packages.mjs` and confirm its
`dirs` are absolute (anchored at `REPO_ROOT`). If they are relative, join them with `REPO_ROOT`
exported from that file instead of `process.cwd()` — the gate must look at the same files from any
working directory.

- [ ] **Step 4: Run the test.** `node --test tests/packages/ruvyxa/package-tiers.test.mjs` —
      Expected: 5 pass.

- [ ] **Step 5: Mark the four packages.** In each `package.json` add, after `"types"`:

```json
  "ruvyxa": { "tier": "experimental" },
```

In each `README.md`, insert directly after the `# @ruvyxa/<name>` title line and its blank line:

```markdown
> **Experimental.** This package ships without a production guarantee: its API and behaviour may
> change in a minor release, and it is not part of the release gate the framework itself passes.
```

- [ ] **Step 6: Wire the gate.** In root `package.json`, insert
      `node scripts/check-package-tiers.mjs && ` into `release:validate` immediately after
      `node scripts/validate-package-metadata.mjs && `.

- [ ] **Step 7: Run** `node scripts/check-package-tiers.mjs` (expected:
      `Package tiers hold across N packages.`), then `pnpm release:validate` and `pnpm check:unused`
      (the new script is a Knip entry by location; the exported `tierFailures` is used by the test).

- [ ] **Step 8: Checkpoint** — report diff and results. Do not commit.

---

### Task 3: Rust image service (sidecar)

**Files:**

- Modify: `crates/ruvyxa_dev_server/src/framework_endpoints.rs:1045-1134`
- Create: `crates/ruvyxa_dev_server/src/image_service.rs`
- Modify: `crates/ruvyxa_dev_server/src/lib.rs` (module list near line 39)

**Interfaces:**

- Produces: `pub const IMAGE_SERVICE_TOKEN_HEADER: &str = "x-ruvyxa-image-token";`
- Produces:
  `pub fn image_service_router(public_dir: PathBuf, config: DynamicImageConfig, token: String) -> axum::Router`
- Produces:
  `pub async fn spawn_image_service(public_dir: PathBuf, config: DynamicImageConfig, token: String) -> std::io::Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)>`
- Produces (crate-private):
  `async fn dynamic_image_response(public_dir: &Path, config: &DynamicImageConfig, cache: &DynamicImageCache, query: &DynamicImageQuery, headers: &HeaderMap) -> Response`

- [ ] **Step 1: Extract the response builder** (pure refactor, no behaviour change). In
      `framework_endpoints.rs` replace the body of `dynamic_image_endpoint` with a call, and move
      the old body into the new function with `state.config.public_dir` → `public_dir`,
      `state.config.dynamic_images` → `config`, `state.dynamic_image_cache` → `cache`, `query.src` →
      `query.src` (via `&query`), and `headers` → `headers`:

```rust
pub(crate) async fn dynamic_image_endpoint(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DynamicImageQuery>,
    headers: HeaderMap,
) -> Response {
    dynamic_image_response(
        &state.config.public_dir,
        &state.config.dynamic_images,
        &state.dynamic_image_cache,
        &query,
        &headers,
    )
    .await
}

/// `/__ruvyxa/image`, answered from whichever host owns the pipeline.
///
/// One function for the Axum host and for the loopback service `ruvyxa start`
/// runs beside the standalone server, so the status a bad request gets, the
/// cache headers, and the ETag cannot differ between them.
pub(crate) async fn dynamic_image_response(
    public_dir: &std::path::Path,
    config: &crate::DynamicImageConfig,
    cache: &crate::dynamic_image::DynamicImageCache,
    query: &DynamicImageQuery,
    headers: &HeaderMap,
) -> Response {
    // ...the previous body of dynamic_image_endpoint, unchanged apart from the
    // five substitutions listed in Step 1...
}
```

The `// ...previous body...` comment above is an instruction for this step only — move the actual
lines (the quality check through the final `with_security_headers(response)`); do not leave the
comment in the file.

- [ ] **Step 2: Prove the refactor is neutral.** Run:
      `cargo test -p ruvyxa_dev_server dynamic_image` Expected: PASS (same count as before the
      change).

- [ ] **Step 3: Write the failing tests** — create `crates/ruvyxa_dev_server/src/image_service.rs`
      with only the test module and the declarations it needs to compile-fail meaningfully:

```rust
//! On-demand image optimization for a standalone server.
//!
//! `ruvyxa start` runs a build on the standalone JavaScript server, which has
//! no image pipeline of its own. Rather than a second pipeline in JavaScript,
//! `start` serves this router on a loopback port and hands the standalone
//! server an `optimizeImage` that forwards here — the shape the Vercel and
//! Cloudflare adapters use to reach their platforms' optimizers.

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn enabled() -> DynamicImageConfig {
        DynamicImageConfig {
            enabled: true,
            ..Default::default()
        }
    }

    fn request(uri: &str, token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(uri);
        if let Some(token) = token {
            builder = builder.header(IMAGE_SERVICE_TOKEN_HEADER, token);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn refuses_a_request_without_the_token() {
        let temp = tempfile::tempdir().unwrap();
        let router = image_service_router(temp.path().to_path_buf(), enabled(), "secret".into());
        for token in [None, Some("wrong"), Some("secre")] {
            let response = router
                .clone()
                .oneshot(request("/__ruvyxa/image?src=/a.png&w=40", token))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "token {token:?}");
        }
    }

    #[tokio::test]
    async fn answers_like_the_axum_host_for_a_missing_source() {
        let temp = tempfile::tempdir().unwrap();
        let router = image_service_router(temp.path().to_path_buf(), enabled(), "secret".into());
        let response = router
            .oneshot(request("/__ruvyxa/image?src=/missing.png&w=40", Some("secret")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn resizes_a_public_image_to_webp() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("avatar.png"),
            crate::image_decode::fixtures::png(
                100,
                50,
                png::ColorType::Rgba,
                png::BitDepth::Eight,
                &[10, 40, 90, 255],
            ),
        )
        .unwrap();
        let router = image_service_router(temp.path().to_path_buf(), enabled(), "secret".into());
        let response = router
            .oneshot(request("/__ruvyxa/image?src=/avatar.png&w=40&q=80", Some("secret")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "image/webp");
        assert!(response.headers().contains_key("etag"));
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(crate::image_decode::header_dimensions(&body).unwrap(), (40, 20));
    }

    #[tokio::test]
    async fn spawned_service_listens_on_loopback() {
        let temp = tempfile::tempdir().unwrap();
        let (address, task) =
            spawn_image_service(temp.path().to_path_buf(), enabled(), "secret".into())
                .await
                .unwrap();
        assert!(address.ip().is_loopback());
        assert_ne!(address.port(), 0);
        task.abort();
    }
}
```

Add `mod image_service;` is not enough — the CLI calls it, so in `lib.rs` next to
`mod dynamic_image;` add `pub mod image_service;`. Confirm `png` and `tempfile` are dev-dependencies
of `ruvyxa_dev_server` (the `dynamic_image` tests already use both).

- [ ] **Step 4: Run to see it fail.** Run: `cargo test -p ruvyxa_dev_server image_service` Expected:
      FAIL — `cannot find function image_service_router`.

- [ ] **Step 5: Implement** — add above the test module:

```rust
use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};

use crate::DynamicImageConfig;
use crate::dynamic_image::DynamicImageCache;
use crate::framework_endpoints::{DynamicImageQuery, dynamic_image_response};

/// The header carrying the per-process token the standalone server presents.
///
/// The service listens on loopback only, but any local process can reach a
/// loopback port, and every request here is a decode and a resize. The token is
/// generated by `ruvyxa start` and handed to its one child.
pub const IMAGE_SERVICE_TOKEN_HEADER: &str = "x-ruvyxa-image-token";

struct ImageServiceState {
    public_dir: PathBuf,
    config: DynamicImageConfig,
    cache: DynamicImageCache,
    token: String,
}

pub fn image_service_router(public_dir: PathBuf, config: DynamicImageConfig, token: String) -> Router {
    let state = Arc::new(ImageServiceState {
        public_dir,
        config,
        cache: DynamicImageCache::default(),
        token,
    });
    Router::new()
        .route("/__ruvyxa/image", get(serve))
        .with_state(state)
}

/// Bind `127.0.0.1:0` and serve the router until the task is dropped.
pub async fn spawn_image_service(
    public_dir: PathBuf,
    config: DynamicImageConfig,
    token: String,
) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let router = image_service_router(public_dir, config, token);
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, router).await {
            tracing::error!(%error, "image service stopped");
        }
    });
    Ok((address, task))
}

async fn serve(
    State(state): State<Arc<ImageServiceState>>,
    Query(query): Query<DynamicImageQuery>,
    headers: HeaderMap,
) -> Response {
    let presented = headers
        .get(IMAGE_SERVICE_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !constant_time_eq(presented.as_bytes(), state.token.as_bytes()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    dynamic_image_response(&state.public_dir, &state.config, &state.cache, &query, &headers).await
}

/// Compare without stopping at the first differing byte.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}
```

If `DynamicImageCache` or `DynamicImageQuery` are not visible from this module, widen them to
`pub(crate)` (they already are on 2026-10-12). Do not make their fields public: the response builder
reads them inside `framework_endpoints.rs`.

- [ ] **Step 6: Run** `cargo test -p ruvyxa_dev_server image_service` — Expected: 4 pass. Then
      `cargo clippy -p ruvyxa_dev_server --locked -- -D warnings` — Expected: clean.

- [ ] **Step 7: Checkpoint** — report diff. Do not commit.

---

### Task 4: Cluster supervision and the image optimizer in the standalone server

**Files:**

- Create: `packages/@ruvyxa/core/src/standalone-cluster.ts`
- Modify: `packages/@ruvyxa/core/src/standalone-server.ts`
- Modify: `packages/@ruvyxa/core/src/index.ts` (export the two rules)
- Test: `tests/packages/core/standalone-cluster.test.ts` (new)
- Test: `tests/packages/core/standalone-server.test.ts` (extend)

**Interfaces:**

- Produces: `resolveWorkerCount(value: string | undefined, parallelism: number): number` — throws
  `Error` whose message starts with `RUV1025` for invalid input.
- Produces:
  `crashLoopExceeded(crashTimes: readonly number[], now: number, limit?: number, windowMs?: number): boolean`
- Produces: `clusterSource(runtime: StandaloneServerRuntime): string` — defines `WORKER_COUNT`,
  `IS_SUPERVISOR`, and `runSupervisor()` in emitted code. Requires `log`, `os`, `process`,
  `SHUTDOWN_GRACE_MS`, `DRAIN_DELAY_MS`, `RUVYXA_RUNTIME` to be defined earlier in the program.
- Produces (emitted): env `RUVYXA_WORKERS`, `RUVYXA_REUSE_PORT`, `RUVYXA_IMAGE_SERVICE`,
  `RUVYXA_IMAGE_SERVICE_TOKEN`.

- [ ] **Step 1: Write the failing rule tests** `tests/packages/core/standalone-cluster.test.ts`:

```ts
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import {
  crashLoopExceeded,
  resolveWorkerCount,
} from '../../../packages/@ruvyxa/core/dist/standalone-cluster.js'

describe('resolveWorkerCount', () => {
  it('defaults to one process', () => {
    assert.equal(resolveWorkerCount(undefined, 8), 1)
    assert.equal(resolveWorkerCount('  ', 8), 1)
  })
  it('reads a positive integer', () => {
    assert.equal(resolveWorkerCount('4', 8), 4)
    assert.equal(resolveWorkerCount(' 2 ', 8), 2)
  })
  it('reads auto as the available parallelism, never below one', () => {
    assert.equal(resolveWorkerCount('auto', 6), 6)
    assert.equal(resolveWorkerCount('AUTO', 0), 1)
  })
  it('refuses anything else with RUV1025', () => {
    for (const value of ['0', '-1', '1.5', 'two', '04']) {
      assert.throws(() => resolveWorkerCount(value, 8), /^Error: RUV1025 /, value)
    }
  })
})

describe('crashLoopExceeded', () => {
  it('allows five crashes inside the window and refuses the sixth', () => {
    const now = 100_000
    const five = [now - 50_000, now - 40_000, now - 30_000, now - 20_000, now - 10_000]
    assert.equal(crashLoopExceeded(five, now), false)
    assert.equal(crashLoopExceeded([...five, now], now), true)
  })
  it('forgets crashes older than the window', () => {
    const now = 200_000
    const old = [1, 2, 3, 4, 5, 6]
    assert.equal(crashLoopExceeded(old, now), false)
  })
})
```

- [ ] **Step 2: Run to see it fail.** Run:
      `pnpm --filter @ruvyxa/core build; node --test tests/packages/core/standalone-cluster.test.ts`
      (use the same runner invocation the core package's `test` script uses — read it from
      `packages/@ruvyxa/core/package.json`). Expected: FAIL — module not found.

- [ ] **Step 3: Implement** `packages/@ruvyxa/core/src/standalone-cluster.ts`:

```ts
import type { StandaloneServerRuntime } from './standalone-server.js'

/**
 * How many request-serving processes `RUVYXA_WORKERS` asks for.
 *
 * Emitted into the generated server with `Function.prototype.toString`, so the
 * function these tests call and the one a deployment runs are the same text.
 * It must therefore stay self-contained: no imports, no closures.
 */
export function resolveWorkerCount(value: string | undefined, parallelism: number): number {
  const raw = (value ?? '').trim().toLowerCase()
  if (raw === '') return 1
  if (raw === 'auto') return Math.max(1, Math.floor(parallelism))
  if (!/^[1-9][0-9]*$/.test(raw)) {
    throw new Error(
      'RUV1025 RUVYXA_WORKERS must be a positive integer or "auto", got "' + String(value) + '"',
    )
  }
  return Number(raw)
}

/**
 * Whether a worker has crashed often enough that respawning is hiding a fault.
 *
 * Self-contained for the same reason as `resolveWorkerCount`. More than
 * `limit` crashes inside `windowMs` makes the supervisor exit non-zero, so the
 * orchestrator in front of it sees a failing process instead of a quiet loop.
 */
export function crashLoopExceeded(
  crashTimes: readonly number[],
  now: number,
  limit = 5,
  windowMs = 60_000,
): boolean {
  return crashTimes.filter((time) => now - time < windowMs).length > limit
}

/**
 * The supervisor half of the generated server.
 *
 * Node uses `node:cluster`, which shares the listening socket on every
 * platform and gives each worker an IPC channel — the channel is how a drain
 * is requested, because on Windows a signal sent to a worker kills it outright.
 * Bun has no cluster primitive, so it spawns processes that listen with
 * `reusePort`, which the kernel only load-balances on Linux; elsewhere more than
 * one worker is refused rather than approximated. Deno has neither, and refuses.
 */
export function clusterSource(runtime: StandaloneServerRuntime): string {
  const rules = `${resolveWorkerCount.toString()}

${crashLoopExceeded.toString()}

let WORKER_COUNT = 1;
try {
  WORKER_COUNT = resolveWorkerCount(process.env.RUVYXA_WORKERS, os.availableParallelism());
} catch (error) {
  log('error', error instanceof Error ? error.message : String(error));
  process.exit(1);
}
`
  if (runtime === 'deno') {
    return `${rules}
if (WORKER_COUNT > 1) {
  log('error', 'RUV1025 RUVYXA_WORKERS above 1 is not supported on Deno; run one process per instance');
  process.exit(1);
}
const IS_SUPERVISOR = false;
function runSupervisor() {}
`
  }

  const spawnWorker =
    runtime === 'node'
      ? `function spawnWorker() {
  return cluster.fork();
}
function stopWorker(worker) {
  // A message, not a signal: on Windows a signal sent to a worker ends it
  // outright, and the drain below is the whole point.
  try {
    worker.send({ ruvyxa: 'shutdown' });
  } catch {
    worker.kill();
  }
}`
      : `function spawnWorker() {
  return spawn(process.execPath, process.argv.slice(1), {
    env: { ...process.env, RUVYXA_WORKERS: '1', RUVYXA_REUSE_PORT: '1' },
    stdio: 'inherit',
  });
}
function stopWorker(worker) {
  worker.kill('SIGTERM');
}`

  const imports =
    runtime === 'node'
      ? "import cluster from 'node:cluster';"
      : "import { spawn } from 'node:child_process';"

  const supervisorAllowed =
    runtime === 'node' ? 'cluster.isPrimary' : "process.env.RUVYXA_REUSE_PORT !== '1'"

  const platformGuard =
    runtime === 'bun'
      ? `  if (process.platform !== 'linux') {
    log('error', 'RUV1025 RUVYXA_WORKERS above 1 needs SO_REUSEPORT load balancing, which Bun gets only on Linux');
    process.exit(1);
  }
`
      : ''

  return `${imports}

${rules}
const IS_SUPERVISOR = WORKER_COUNT > 1 && ${supervisorAllowed};

${spawnWorker}

function runSupervisor() {
${platformGuard}  const crashes = [];
  let live = 0;
  let stopping = false;
  const workers = new Set();

  function start() {
    const worker = spawnWorker();
    live += 1;
    workers.add(worker);
    worker.on('exit', (code, signal) => {
      live -= 1;
      workers.delete(worker);
      if (stopping) {
        if (live === 0) process.exit(0);
        return;
      }
      const now = Date.now();
      crashes.push(now);
      if (crashLoopExceeded(crashes, now)) {
        log('error', 'worker crash loop, exiting', { crashes: crashes.length });
        process.exit(1);
      }
      const delay = Math.min(30_000, 250 * 2 ** Math.min(crashes.length, 7));
      log('warn', 'worker exited, respawning', { code, signal, delay_ms: delay });
      setTimeout(start, delay);
    });
  }

  for (let index = 0; index < WORKER_COUNT; index += 1) start();

  function stop(reason) {
    if (stopping) {
      log('warn', 'shutdown forced', { reason });
      process.exit(1);
    }
    stopping = true;
    log('info', 'stopping workers', { reason, workers: live });
    for (const worker of workers) stopWorker(worker);
    const forceExit = setTimeout(() => process.exit(1), SHUTDOWN_GRACE_MS + DRAIN_DELAY_MS);
    if (typeof forceExit?.unref === 'function') forceExit.unref();
  }
  for (const signal of ['SIGTERM', 'SIGINT']) {
    try {
      process.on(signal, () => stop('received ' + signal));
    } catch {
      // this runtime and platform do not deliver that signal
    }
  }
  log('info', 'supervising workers', { workers: WORKER_COUNT, runtime: RUVYXA_RUNTIME });
}
`
}
```

Export `resolveWorkerCount` and `crashLoopExceeded` from `packages/@ruvyxa/core/src/index.ts` only
if the core package's `exports` map does not already expose `./dist/standalone-cluster.js` to the
test by path — the existing `standalone-server.test.ts` imports `dist/standalone-server.js` by path,
so follow that and do **not** widen the public API. `pnpm check:unused` decides: if Knip flags the
two exports as unused because only a test imports them, add the file to the same Knip entry list
that covers `standalone-server.ts`.

- [ ] **Step 4: Run the rule tests.** Build core, re-run Step 2's command. Expected: 6 pass.

- [ ] **Step 5: Write the failing emitted-source tests** — append to
      `tests/packages/core/standalone-server.test.ts`, inside the existing `describe`:

```ts
it('emits the cluster supervisor for node and bun, and the refusal for deno', () => {
  const node = standaloneServerSource({ runtime: 'node', runtimePolicy: {} })
  assert.match(node, /import cluster from 'node:cluster';/)
  assert.match(node, /const IS_SUPERVISOR = WORKER_COUNT > 1 && cluster\.isPrimary;/)
  assert.match(node, /if \(IS_SUPERVISOR\) \{\s*runSupervisor\(\);/)

  const bun = standaloneServerSource({ runtime: 'bun', runtimePolicy: {} })
  assert.match(bun, /reusePort: process\.env\.RUVYXA_REUSE_PORT === '1'/)
  assert.match(bun, /needs SO_REUSEPORT load balancing/)

  const deno = standaloneServerSource({ runtime: 'deno', runtimePolicy: {} })
  assert.match(deno, /RUVYXA_WORKERS above 1 is not supported on Deno/)
  assert.doesNotMatch(deno, /node:cluster/)
})

it('forwards /__ruvyxa/image to the image service only when onDemand is on', () => {
  const source = standaloneServerSource({ runtimePolicy: { image: { onDemand: true } } })
  assert.match(source, /process\.env\.RUVYXA_IMAGE_SERVICE/)
  assert.match(source, /'x-ruvyxa-image-token'/)
  assert.match(source, /optimizeImage: imageServiceOptimizer,/)
})
```

Run the core tests. Expected: the two new tests FAIL; the existing `node --check` test still passes.

- [ ] **Step 6: Wire the supervisor into `standalone-server.ts`.**

  1. `import { clusterSource } from './standalone-cluster.js'` at the top of the file.
  2. In `sharedServerSource`, emit `${clusterSource(runtime)}` on the line after the
     `DRAIN_DELAY_MS` constant (it needs `log`, `os`, `process`, `SHUTDOWN_GRACE_MS`,
     `DRAIN_DELAY_MS`, `RUVYXA_RUNTIME`, all defined above that point). `sharedServerSource` already
     receives `runtime`.
  3. Node transport tail (currently `onShutdownSignal(shutdown);` … `server.listen(…)` at the end of
     `nodeTransport()`): wrap the signal registration, both `process.on` handlers, and
     `server.listen` so the supervisor binds nothing:

```js
if (IS_SUPERVISOR) {
  runSupervisor();
} else {
  onShutdownSignal(shutdown);

  // A cluster worker is asked to drain over IPC rather than by signal; see
  // \`stopWorker\`. Outside a cluster nothing ever sends this message.
  process.on('message', (message) => {
    if (message && message.ruvyxa === 'shutdown') shutdown('supervisor requested shutdown', 0);
  });

  // ...the existing unhandledRejection and uncaughtException handlers, unchanged...

  server.listen(port, host, () => {
    log('info', 'listening on', {
      runtime: RUVYXA_RUNTIME,
      url: \`http://\${host === '0.0.0.0' ? 'localhost' : host}:\${port}\`,
    });
  });
}
```

Keep the backslash escapes exactly as the surrounding template uses them (this is inside a template
literal). Move the two existing handlers into the `else` block — do not duplicate them.

4. Fetch transport, Bun branch: change `const server = Bun.serve({` to
   `const server = IS_SUPERVISOR ? null : Bun.serve({` and add
   `reusePort: process.env.RUVYXA_REUSE_PORT === '1',` after `hostname: host,`. Change `closeServer`
   to `return server?.stop(false);`.
5. Fetch transport tail: wrap `onShutdownSignal(shutdown);`, both `process.on` handlers, and
   `${banner}` in the same `if (IS_SUPERVISOR) { runSupervisor(); } else { … }` shape. For Deno,
   `IS_SUPERVISOR` is the constant `false`, so the Deno program is unchanged in behaviour.

- [ ] **Step 7: Add the image-service optimizer** in `sharedServerSource`, immediately before
      `const handler = createHandler({`:

```js
// \`ruvyxa start\` serves the Rust image pipeline on a loopback port and names it
// here; a deployment started any other way has neither variable, passes no
// optimizer, and answers /__ruvyxa/image with 404 as it always has. One
// pipeline, reached the way the Vercel and Cloudflare adapters reach theirs.
const IMAGE_SERVICE = (process.env.RUVYXA_IMAGE_SERVICE ?? '').trim()
const IMAGE_SERVICE_TOKEN = process.env.RUVYXA_IMAGE_SERVICE_TOKEN ?? ''
const imageServiceOptimizer =
  runtimePolicy.image?.onDemand === true && IMAGE_SERVICE !== ''
    ? async (request, { src, width, quality }) => {
        const target = new URL('/__ruvyxa/image', IMAGE_SERVICE)
        target.searchParams.set('src', src)
        target.searchParams.set('w', String(width))
        target.searchParams.set('q', String(quality))
        const headers = { 'x-ruvyxa-image-token': IMAGE_SERVICE_TOKEN }
        const ifNoneMatch = request.headers.get('if-none-match')
        if (ifNoneMatch) headers['if-none-match'] = ifNoneMatch
        const answer = await fetch(target, { headers })
        return new Response(request.method === 'HEAD' ? null : answer.body, {
          status: answer.status,
          headers: answer.headers,
        })
      }
    : undefined
```

and add `  optimizeImage: imageServiceOptimizer,` to the `createHandler({` options, after
`security: runtimePolicy.security,`.

- [ ] **Step 8: Run the core tests.** Expected: all pass, including the `node --check` test for all
      three runtimes and both ISR modes (this is what catches an unescaped backtick).

- [ ] **Step 9: Run** `pnpm --filter @ruvyxa/core check`, `pnpm lint`, `pnpm check:unused`.
      Expected: clean.

- [ ] **Step 10: Checkpoint** — report diff. Do not commit.

---

### Task 5: Every build leaves a stamped standalone server

**Files:**

- Modify: `crates/ruvyxa_cli/src/runtime_config.rs:457-490` (extract `manifest_build_id`)
- Create: `crates/ruvyxa_cli/src/start.rs` (path and stamp helpers only in this task)
- Modify: `crates/ruvyxa_cli/src/main.rs` (`mod start;`)
- Modify: `crates/ruvyxa_cli/src/build.rs:1417-1426`
- Test: `crates/ruvyxa_cli/src/tests.rs`

**Interfaces:**

- Produces: `pub(crate) fn manifest_build_id(out_dir: &Path) -> Option<String>` (runtime_config.rs)
- Produces (start.rs): `pub(crate) const STANDALONE_BUILD_ID_FILE: &str = ".ruvyxa-build-id";`
- Produces: `pub(crate) fn standalone_dir(out_dir: &Path, runtime: JavaScriptRuntime) -> PathBuf`
- Produces:
  `pub(crate) fn local_standalone_adapter(runtime: JavaScriptRuntime, selected: Option<&str>) -> Option<&'static str>`
  — `None` when `selected` already equals `runtime.command()`, else `Some(runtime.command())`.
- Produces:
  `pub(crate) fn stamp_standalone(out_dir: &Path, runtime: JavaScriptRuntime) -> anyhow::Result<()>`
- Produces: `pub(crate) enum StandaloneState { Current, Missing, Stale }` and
  `pub(crate) fn standalone_state(out_dir: &Path, runtime: JavaScriptRuntime) -> StandaloneState`

- [ ] **Step 1: Write the failing tests** in `crates/ruvyxa_cli/src/tests.rs` (append at the end of
      the module, using the same `tempfile` and `fs` imports the file already uses):

```rust
#[test]
fn manifest_build_id_reads_deploy_build_id_and_ignores_an_empty_one() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(crate::runtime_config::manifest_build_id(temp.path()), None);
    std::fs::write(temp.path().join("manifest.json"), r#"{"deploy":{"buildId":""}}"#).unwrap();
    assert_eq!(crate::runtime_config::manifest_build_id(temp.path()), None);
    std::fs::write(temp.path().join("manifest.json"), r#"{"deploy":{"buildId":"abc123"}}"#).unwrap();
    assert_eq!(
        crate::runtime_config::manifest_build_id(temp.path()).as_deref(),
        Some("abc123")
    );
}

#[test]
fn the_local_standalone_stage_runs_only_when_the_selected_adapter_is_not_the_runtime() {
    use crate::start::local_standalone_adapter;
    use ruvyxa_dev_server::JavaScriptRuntime;
    assert_eq!(local_standalone_adapter(JavaScriptRuntime::Node, None), Some("node"));
    assert_eq!(local_standalone_adapter(JavaScriptRuntime::Node, Some("vercel")), Some("node"));
    assert_eq!(local_standalone_adapter(JavaScriptRuntime::Node, Some("node")), None);
    assert_eq!(local_standalone_adapter(JavaScriptRuntime::Bun, Some("node")), Some("bun"));
    assert_eq!(local_standalone_adapter(JavaScriptRuntime::Deno, Some("deno")), None);
}

#[test]
fn a_standalone_server_is_current_only_with_a_matching_stamp() {
    use crate::start::{StandaloneState, stamp_standalone, standalone_dir, standalone_state};
    use ruvyxa_dev_server::JavaScriptRuntime;
    let temp = tempfile::tempdir().unwrap();
    let out = temp.path();
    let runtime = JavaScriptRuntime::Node;
    assert!(matches!(standalone_state(out, runtime), StandaloneState::Missing));

    let server = standalone_dir(out, runtime).join("server");
    std::fs::create_dir_all(&server).unwrap();
    std::fs::write(server.join("index.mjs"), "").unwrap();
    std::fs::write(out.join("manifest.json"), r#"{"deploy":{"buildId":"one"}}"#).unwrap();
    assert!(matches!(standalone_state(out, runtime), StandaloneState::Stale));

    stamp_standalone(out, runtime).unwrap();
    assert!(matches!(standalone_state(out, runtime), StandaloneState::Current));

    std::fs::write(out.join("manifest.json"), r#"{"deploy":{"buildId":"two"}}"#).unwrap();
    assert!(matches!(standalone_state(out, runtime), StandaloneState::Stale));
}
```

- [ ] **Step 2: Run to see it fail.** Run: `cargo test -p ruvyxa_cli standalone` and
      `cargo test -p ruvyxa_cli manifest_build_id` Expected: FAIL — unresolved `crate::start`,
      `manifest_build_id`.

- [ ] **Step 3: Extract `manifest_build_id`** in `runtime_config.rs` and make
      `data_cache_key_prefix` use it:

```rust
/// `manifest.json`'s `deploy.buildId`, or `None` when the file, the field, or
/// a non-empty value is missing. The one reader of that field on this side.
pub(crate) fn manifest_build_id(out_dir: &Path) -> Option<String> {
    fs::read_to_string(out_dir.join("manifest.json"))
        .ok()
        .and_then(|source| serde_json::from_str::<serde_json::Value>(&source).ok())
        .and_then(|manifest| {
            manifest
                .get("deploy")?
                .get("buildId")?
                .as_str()
                .map(str::to_string)
        })
        .filter(|id| !id.is_empty())
}
```

In `data_cache_key_prefix`, replace the `let build_id = fs::read_to_string(...)...;` expression with
`let manifest_path = out_dir.join("manifest.json"); let build_id = manifest_build_id(out_dir);` and
leave the rest unchanged.

- [ ] **Step 4: Create `start.rs`** with the helpers:

```rust
//! `ruvyxa start`: run a build on the standalone server.
//!
//! Production has one request host — `createHandler` inside the standalone
//! server every self-hosted adapter emits. `start` used to serve the build
//! through the Axum host instead, which made it a second production host that
//! had to agree with the first through a table of conformance fixtures. It is a
//! supervisor now: it finds the standalone server the build left, checks it
//! belongs to this build, and runs it.

use std::path::{Path, PathBuf};

use anyhow::Context;
use ruvyxa_dev_server::JavaScriptRuntime;

use crate::runtime_config::manifest_build_id;

/// The file a staged standalone server records its build id in.
pub(crate) const STANDALONE_BUILD_ID_FILE: &str = ".ruvyxa-build-id";

/// Where the standalone server for `runtime` lives in the build output: the
/// directory the built-in adapter of the same name writes.
pub(crate) fn standalone_dir(out_dir: &Path, runtime: JavaScriptRuntime) -> PathBuf {
    out_dir.join("deploy").join(runtime.command())
}

/// The built-in adapter to run for `start`, or `None` when the build's own
/// adapter stage already produced that directory.
pub(crate) fn local_standalone_adapter(
    runtime: JavaScriptRuntime,
    selected: Option<&str>,
) -> Option<&'static str> {
    (selected != Some(runtime.command())).then_some(runtime.command())
}

/// Record which build the staged server belongs to.
pub(crate) fn stamp_standalone(out_dir: &Path, runtime: JavaScriptRuntime) -> anyhow::Result<()> {
    let build_id = manifest_build_id(out_dir)
        .context("the build output has no deploy.buildId to stamp the standalone server with")?;
    let stamp = standalone_dir(out_dir, runtime).join(STANDALONE_BUILD_ID_FILE);
    ruvyxa_bundler::atomic_file::write_atomic(&stamp, build_id.as_bytes())
        .with_context(|| format!("failed to write {}", stamp.display()))
}

pub(crate) enum StandaloneState {
    Current,
    Missing,
    Stale,
}

pub(crate) fn standalone_state(out_dir: &Path, runtime: JavaScriptRuntime) -> StandaloneState {
    let dir = standalone_dir(out_dir, runtime);
    if !dir.join("server").join("index.mjs").is_file() {
        return StandaloneState::Missing;
    }
    let stamped = std::fs::read_to_string(dir.join(STANDALONE_BUILD_ID_FILE))
        .ok()
        .map(|value| value.trim().to_string());
    match (stamped, manifest_build_id(out_dir)) {
        (Some(stamped), Some(current)) if stamped == current => StandaloneState::Current,
        _ => StandaloneState::Stale,
    }
}
```

Add `mod start;` to `main.rs` in alphabetical position (after `mod site_discovery;`).

- [ ] **Step 5: Run the three tests.** Expected: pass.

- [ ] **Step 6: Run the local stage in the build.** In `build.rs`, after the
      `if let Some(artifacts) =     run_adapter_stage(...) { ... }` block (line 1426) and before
      `build_info["timing"]["totalMs"]`, add:

```rust
    run_local_standalone_stage(&args, &config, &out_dir, &detected_adapter)?;
```

and add the function beside `run_adapter_stage`:

```rust
/// Leave a standalone server for the configured runtime in every build.
///
/// `ruvyxa start` runs it, so it has to exist whichever adapter the build was
/// for — and it is produced by the same built-in adapter a self-hosted
/// deployment uses, not by a second generator. When the build's own adapter is
/// already that one, its output is the standalone server and only the stamp is
/// written. The artifact report of this pass is not recorded in `build.json`:
/// it is a local convenience, not a deployment the project asked for.
fn run_local_standalone_stage(
    args: &BuildArgs,
    config: &ProjectConfig,
    out_dir: &Path,
    detected_adapter: &Option<(String, String)>,
) -> anyhow::Result<()> {
    let runtime = config.javascript_runtime();
    let selected = args
        .adapter
        .as_deref()
        .or_else(|| config.adapter.as_ref().and_then(serde_json::Value::as_str))
        .or_else(|| detected_adapter.as_ref().map(|(name, _)| name.as_str()));
    if let Some(adapter) = crate::start::local_standalone_adapter(runtime, selected) {
        run_adapter_runner(&args.root, out_dir, runtime, Some(adapter))
            .with_context(|| format!("failed to stage the local {adapter} standalone server"))?;
    }
    crate::start::stamp_standalone(out_dir, runtime)
}
```

- [ ] **Step 7: Prove it on the real fixtures.**

```bash
cargo run -p ruvyxa_cli -- build --root examples/deploy-smoke
```

Expected: `examples/deploy-smoke/.ruvyxa/deploy/node/server/index.mjs` and
`examples/deploy-smoke/.ruvyxa/deploy/node/.ruvyxa-build-id` exist, the stamp equals
`deploy.buildId` in `examples/deploy-smoke/.ruvyxa/manifest.json`, and `build.json` has no
`adapterArtifacts` key (no adapter configured).

```bash
cargo run -p ruvyxa_cli -- build --root examples/deploy-smoke --target static
```

**STOP condition:** if the node adapter refuses the static target's output, do not work around it —
report the error to the owner; `start` on a static build then needs a decision (serve via the
`ruvyxa-axum` crate, or refuse with `RUV1022`).

```bash
cargo run -p ruvyxa_cli -- build --root examples/demo
```

Expected: succeeds; `examples/demo/.ruvyxa/deploy/node/.ruvyxa-build-id` exists.

- [ ] **Step 8: Run** `cargo test -p ruvyxa_cli --locked` and
      `cargo clippy -p ruvyxa_cli --locked -- -D warnings`. Some existing tests assert the exact set
      of files a build writes; update only assertions that now legitimately include
      `deploy/<runtime>/`, and list each one in the report.

- [ ] **Step 9: Checkpoint** — report diff. Do not commit.

---

### Task 6: The `start` launcher

**Files:**

- Modify: `crates/ruvyxa_cli/src/start.rs`
- Modify: `crates/ruvyxa_cli/src/main.rs` (`StartArgs`, `Command::Start`/`Preview`, dispatch)
- Modify: `crates/ruvyxa_cli/src/runtime_config.rs:956` (`command_runtime`)
- Modify: every `Command::Start(` / `Command::Preview(` match
  (`rg "Command::(Start|Preview)\(" crates`)
- Test: `crates/ruvyxa_cli/src/tests.rs`

**Interfaces:**

- Consumes: Task 3 `ruvyxa_dev_server::image_service::spawn_image_service`; Task 5 helpers.
- Produces: `StartArgs { server: ServerArgs, workers: Option<String>, legacy_host: bool }`
- Produces:
  `pub(crate) struct LaunchPlan { program: PathBuf, args: Vec<String>, cwd: PathBuf, env: BTreeMap<String, String> }`
- Produces:
  `pub(crate) fn launch_plan(runtime: JavaScriptRuntime, standalone: &Path, host: &str, port: u16, workers: Option<&str>, image_service: Option<(&str, &str)>, project_env: BTreeMap<String, String>) -> LaunchPlan`
- Produces: `pub(crate) async fn start(args: StartArgs) -> anyhow::Result<()>`
- Diagnostics: `RUV1022` (standalone missing/stale), `RUV1023` (runtime missing), `RUV1024`
  (realtime needs `--legacy-host`).

- [ ] **Step 1: Write the failing tests** (append to `tests.rs`):

```rust
#[test]
fn start_parses_workers_and_legacy_host() {
    let cli = Cli::try_parse_from(normalized_cli_args(os_args([
        "Ruvyxa", "start", "--workers", "auto", "--legacy-host", "--port", "4000",
    ])))
    .unwrap();
    let Command::Start(args) = cli.command else {
        panic!("expected start");
    };
    assert_eq!(args.workers.as_deref(), Some("auto"));
    assert!(args.legacy_host);
    assert_eq!(args.server.port, Some(4000));
}

#[test]
fn start_refuses_an_invalid_worker_count_at_parse_time() {
    for value in ["0", "-2", "many"] {
        assert!(
            Cli::try_parse_from(normalized_cli_args(os_args(["Ruvyxa", "start", "--workers", value])))
                .is_err(),
            "{value}"
        );
    }
}

#[test]
fn the_launch_plan_runs_the_standalone_entry_with_the_bind_address_and_services() {
    use ruvyxa_dev_server::JavaScriptRuntime;
    let standalone = std::path::Path::new("/app/.ruvyxa/deploy/node");
    let mut project_env = std::collections::BTreeMap::new();
    project_env.insert("SECRET".to_string(), "value".to_string());
    let plan = crate::start::launch_plan(
        JavaScriptRuntime::Node,
        standalone,
        "127.0.0.1",
        4100,
        Some("2"),
        Some(("http://127.0.0.1:5555", "token")),
        project_env,
    );
    assert_eq!(plan.args, vec!["server/index.mjs".to_string()]);
    assert_eq!(plan.cwd, standalone);
    assert_eq!(plan.env["HOST"], "127.0.0.1");
    assert_eq!(plan.env["PORT"], "4100");
    assert_eq!(plan.env["RUVYXA_WORKERS"], "2");
    assert_eq!(plan.env["RUVYXA_IMAGE_SERVICE"], "http://127.0.0.1:5555");
    assert_eq!(plan.env["RUVYXA_IMAGE_SERVICE_TOKEN"], "token");
    assert_eq!(plan.env["SECRET"], "value");
}

#[test]
fn the_deno_launch_plan_grants_the_permissions_the_smoke_lane_proves() {
    use ruvyxa_dev_server::JavaScriptRuntime;
    let plan = crate::start::launch_plan(
        JavaScriptRuntime::Deno,
        std::path::Path::new("/app/.ruvyxa/deploy/deno"),
        "0.0.0.0",
        3000,
        None,
        None,
        std::collections::BTreeMap::new(),
    );
    assert_eq!(plan.args, ["run", "-A", "--no-prompt", "server/index.mjs"]);
    assert!(!plan.env.contains_key("RUVYXA_WORKERS"));
    assert!(!plan.env.contains_key("RUVYXA_IMAGE_SERVICE"));
}

#[test]
fn start_diagnostics_carry_their_codes() {
    use ruvyxa_dev_server::JavaScriptRuntime;
    let out = std::path::Path::new("/app/.ruvyxa");
    let rendered = crate::start::standalone_not_current(out, JavaScriptRuntime::Node, false).to_string();
    assert!(rendered.contains("RUV1022"), "{rendered}");
    assert!(rendered.contains("ruvyxa build"), "{rendered}");
    let rendered = crate::start::runtime_missing(JavaScriptRuntime::Bun).to_string();
    assert!(rendered.contains("RUV1023") && rendered.contains("bun"), "{rendered}");
    let rendered = crate::start::realtime_needs_legacy_host().to_string();
    assert!(rendered.contains("RUV1024") && rendered.contains("--legacy-host"), "{rendered}");
}
```

- [ ] **Step 2: Run to see it fail.** `cargo test -p ruvyxa_cli start_` and
      `cargo test -p ruvyxa_cli launch_plan` — Expected: compile errors naming the missing items.

- [ ] **Step 3: Add `StartArgs`** in `main.rs` after `ServerArgs`:

```rust
#[derive(Debug, Parser)]
struct StartArgs {
    #[command(flatten)]
    server: ServerArgs,

    /// Request-serving processes: a positive integer or `auto` (one per CPU).
    /// Also read from RUVYXA_WORKERS by the standalone server itself.
    #[arg(long, value_parser = parse_worker_count)]
    workers: Option<String>,

    /// Serve through the previous Axum production host. Deprecated: kept for
    /// one minor release as a rollback, then removed.
    #[arg(long)]
    legacy_host: bool,
}

fn parse_worker_count(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    let valid = trimmed.eq_ignore_ascii_case("auto")
        || (!trimmed.starts_with('0') && trimmed.parse::<u32>().is_ok_and(|count| count > 0));
    if valid {
        Ok(trimmed.to_ascii_lowercase())
    } else {
        Err("expected a positive integer or `auto`".to_string())
    }
}
```

Change `Start(ServerArgs)` and `Preview(ServerArgs)` to `Start(StartArgs)` and `Preview(StartArgs)`.
Change the dispatch arm at `main.rs:492` to:

```rust
        Command::Start(args) | Command::Preview(args) => {
            crate::start::start(args).await?;
        }
```

In `runtime_config.rs:956` split the arm:

```rust
        Command::Dev(args) => args.runtime,
        Command::Start(args) | Command::Preview(args) => args.server.runtime,
```

Then `cargo check -p ruvyxa_cli --tests` and fix each remaining `Command::Start(`/`Preview(` site by
reaching through `.server`.

- [ ] **Step 4: Implement the launcher** — append to `start.rs`:

```rust
use std::collections::BTreeMap;

use ruvyxa_diagnostics::{Diagnostic, RuvyxaError};

use crate::runtime_config::{
    ensure_build_output_exists, production_server_config, resolve_bind_address,
};
use crate::{ProjectConfig, StartArgs, load_project_config, serve, warn_text};

pub(crate) struct LaunchPlan {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) env: BTreeMap<String, String>,
}

/// Arguments before the entry point. Deno's are the ones the deno smoke lane
/// runs a deployment with; `script_args` carries `--node-modules-dir=manual`,
/// which is for tool processes inside a project, not for a self-contained
/// deployment directory.
fn entry_args(runtime: JavaScriptRuntime) -> &'static [&'static str] {
    match runtime {
        JavaScriptRuntime::Node | JavaScriptRuntime::Bun => &[],
        JavaScriptRuntime::Deno => &["run", "-A", "--no-prompt"],
    }
}

pub(crate) fn launch_plan(
    runtime: JavaScriptRuntime,
    standalone: &Path,
    host: &str,
    port: u16,
    workers: Option<&str>,
    image_service: Option<(&str, &str)>,
    project_env: BTreeMap<String, String>,
) -> LaunchPlan {
    let mut args: Vec<String> = entry_args(runtime).iter().map(|arg| (*arg).to_string()).collect();
    args.push("server/index.mjs".to_string());
    let mut env = project_env;
    env.insert("HOST".to_string(), host.to_string());
    env.insert("PORT".to_string(), port.to_string());
    if let Some(workers) = workers {
        env.insert("RUVYXA_WORKERS".to_string(), workers.to_string());
    }
    if let Some((url, token)) = image_service {
        env.insert("RUVYXA_IMAGE_SERVICE".to_string(), url.to_string());
        env.insert("RUVYXA_IMAGE_SERVICE_TOKEN".to_string(), token.to_string());
    }
    LaunchPlan {
        program: runtime.executable(),
        args,
        cwd: standalone.to_path_buf(),
        env,
    }
}

pub(crate) fn standalone_not_current(
    out_dir: &Path,
    runtime: JavaScriptRuntime,
    missing: bool,
) -> Diagnostic {
    let dir = standalone_dir(out_dir, runtime);
    let explain = if missing {
        format!("`ruvyxa start` runs the standalone server the build leaves at `{}`, and it is not there.", dir.display())
    } else {
        format!("The standalone server at `{}` was staged by a different build than the one in `{}`, so it would serve old code.", dir.display(), out_dir.display())
    };
    Diagnostic::new("RUV1022", "Standalone server is missing or older than the build output")
        .explain(explain)
        .at_file(&dir)
        .suggest("Run `ruvyxa build` again, then `ruvyxa start`.")
}

pub(crate) fn runtime_missing(runtime: JavaScriptRuntime) -> Diagnostic {
    Diagnostic::new("RUV1023", "The JavaScript runtime `ruvyxa start` needs was not found")
        .explain(format!(
            "The build is configured for `{}`, and `{}` could not be executed.",
            runtime.command(),
            runtime.executable().display()
        ))
        .suggest(format!(
            "Install {0}, or put it on PATH, or start with `--runtime` naming one that is installed and rebuild for it.",
            runtime.command()
        ))
}

pub(crate) fn realtime_needs_legacy_host() -> Diagnostic {
    Diagnostic::new("RUV1024", "Realtime is served only by the legacy host")
        .explain("`ruvyxa.config.ts` configures realtime, whose WebSocket transport runs on the Axum host. `ruvyxa start` now runs the standalone server, which does not carry it.")
        .suggest("Run `ruvyxa start --legacy-host` for now. `@ruvyxa/realtime` is experimental.")
}

/// The one conversion every refusal below goes through, as
/// `ensure_build_output_exists` does it.
fn refuse(diagnostic: Diagnostic) -> anyhow::Error {
    RuvyxaError::from(diagnostic).into()
}

/// A token only this process and its child know, from the OS-seeded keys
/// `RandomState` draws on each construction.
fn image_service_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut token = String::with_capacity(32);
    for _ in 0..2 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default());
        token.push_str(&format!("{:016x}", hasher.finish()));
    }
    token
}

pub(crate) async fn start(args: StartArgs) -> anyhow::Result<()> {
    let config: ProjectConfig = load_project_config(&args.server.root)?;
    ensure_build_output_exists(&args.server, &config)?;

    if args.legacy_host {
        eprintln!(
            "{}",
            warn_text("`ruvyxa start --legacy-host` serves through the previous host and will be removed in the next minor release.")
        );
        return serve(production_server_config(&args.server, &config)?)
            .await
            .context("production server failed");
    }

    if config.realtime.is_some() {
        return Err(refuse(realtime_needs_legacy_host()));
    }

    let server = production_server_config(&args.server, &config)?;
    let runtime = server.runtime;
    let out_dir = args.server.root.join(config.out_dir());
    match standalone_state(&out_dir, runtime) {
        StandaloneState::Current => {}
        StandaloneState::Missing => return Err(refuse(standalone_not_current(&out_dir, runtime, true))),
        StandaloneState::Stale => return Err(refuse(standalone_not_current(&out_dir, runtime, false))),
    }
    if !runtime.is_available() {
        return Err(refuse(runtime_missing(runtime)));
    }

    let (host, port) = resolve_bind_address(
        &args.server,
        &config,
        |key| std::env::var(key).ok(),
        crate::runtime_config::PRODUCTION_DEFAULT_HOST,
    )?;

    let token = image_service_token();
    let mut _image_task = None;
    let image_url = if server.dynamic_images.enabled {
        let (address, task) = ruvyxa_dev_server::image_service::spawn_image_service(
            server.public_dir.clone(),
            server.dynamic_images.clone(),
            token.clone(),
        )
        .await
        .context("failed to start the image service")?;
        _image_task = Some(task);
        Some(format!("http://{address}"))
    } else {
        None
    };

    let plan = launch_plan(
        runtime,
        &standalone_dir(&out_dir, runtime),
        &host,
        port,
        args.workers.as_deref(),
        image_url.as_deref().map(|url| (url, token.as_str())),
        ruvyxa_dev_server::project_env(&args.server.root)?,
    );
    let code = supervise(plan).await?;
    std::process::exit(code);
}

/// Run the child to completion, forwarding termination, and return its code.
///
/// On unix the child gets its own process group, so a terminal's Ctrl-C reaches
/// only this process and is forwarded once — the standalone server treats a
/// second signal as "exit now", and a group-wide SIGINT plus a forwarded one
/// would be exactly that. On Windows the console delivers Ctrl-C to both
/// processes and there is nothing to forward.
async fn supervise(plan: LaunchPlan) -> anyhow::Result<i32> {
    let mut command = tokio::process::Command::new(&plan.program);
    command
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .envs(&plan.env)
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to run {}", plan.program.display()))?;

    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate())?;
        let mut interrupt = signal(SignalKind::interrupt())?;
        loop {
            tokio::select! {
                status = child.wait() => return Ok(exit_code(status?)),
                _ = terminate.recv() => forward(&child, libc::SIGTERM),
                _ = interrupt.recv() => forward(&child, libc::SIGINT),
            }
        }
    }
    #[cfg(not(unix))]
    {
        loop {
            tokio::select! {
                status = child.wait() => return Ok(exit_code(status?)),
                _ = tokio::signal::ctrl_c() => {}
            }
        }
    }
}

#[cfg(unix)]
fn forward(child: &tokio::process::Child, signal: libc::c_int) {
    let Some(pid) = child.id().and_then(|pid| libc::pid_t::try_from(pid).ok()) else {
        return;
    };
    // SAFETY: kill(2) on a pid this process spawned and is still waiting on, so
    // it cannot have been reused; no memory is read or written.
    unsafe {
        libc::kill(pid, signal);
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    status.code().unwrap_or(1)
}
```

Adjust the `use crate::...` line to the real paths: `load_project_config`, `serve`, `warn_text`,
`ProjectConfig`, `PRODUCTION_DEFAULT_HOST`, and whether `resolve_bind_address` /
`ensure_build_output_exists` are reachable from `crate::runtime_config` — `cargo check` names each.
If `PRODUCTION_DEFAULT_HOST` is private, make it `pub(crate)`. The three diagnostic helpers return
`Diagnostic` (which the tests render with `to_string()`, as the existing `RUV1211` test does) and
reach the caller through `refuse`.

- [ ] **Step 5: Run the new tests.** Expected: 5 pass. Then the whole crate:
      `cargo test -p ruvyxa_cli --locked` and `cargo test -p ruvyxa_diagnostics --locked` (the
      one-meaning gate must accept the four new titles). Then
      `cargo clippy --workspace --locked -- -D warnings`.

- [ ] **Step 6: Try it by hand** (Node, Windows host):

```bash
cargo run -p ruvyxa_cli -- build --root examples/deploy-smoke
cargo run -p ruvyxa_cli -- start --root examples/deploy-smoke --port 4410
```

In another terminal: `curl -s http://127.0.0.1:4410/__ruvyxa/health` — Expected:
`{"status":"ok","host":"node"}`. Stop with Ctrl-C — Expected: `draining connections` then
`shutdown complete` in the output, exit code 0. Then delete
`examples/deploy-smoke/.ruvyxa/deploy/node/.ruvyxa-build-id` and start again — Expected: `RUV1022`.

- [ ] **Step 7: Checkpoint** — report diff and the manual results. Do not commit.

---

### Task 7: The `start` smoke lane, cluster behaviour, and CI

**Files:**

- Modify: `scripts/smoke-runtime-adapter.mjs`
- Modify: `.github/workflows/ci.yml` (after the "Build and launch Node deployment artifact" step)

**Interfaces:**

- Consumes: `RUVYXA_CLI` (path to the built `ruvyxa` binary, as `scripts/smoke-dev-server.mjs`
  uses).
- Produces: lane `start` taking `<project-root> <port>`; env `RUVYXA_SMOKE_WORKERS` (default `2`).

- [ ] **Step 1: Add the lane.** In `scripts/smoke-runtime-adapter.mjs`:
  - Append `'start'` to `RUNTIMES` and to `SPAWNED`.
  - Add, beside `startAxumHost`:

```js
/**
 * Run `ruvyxa start` itself rather than the program it launches.
 *
 * The node lane proves the standalone server; this one proves that `start`
 * finds the server the build left, passes the bind address through, and puts
 * the image service and the worker count in front of it — the launcher is the
 * part no other lane touches. Two workers by default, so every check below is
 * also a check that a cluster answers like a single process.
 */
function startRuvyxaStart() {
  const program = process.env.RUVYXA_CLI
  if (!program) throw new Error('RUVYXA_CLI must name the built ruvyxa binary')
  child = spawn(
    program,
    [
      'start',
      '--root',
      path.resolve(deploymentDirectory),
      '--host',
      '127.0.0.1',
      '--port',
      String(innerPort),
      '--workers',
      process.env.RUVYXA_SMOKE_WORKERS ?? '2',
    ],
    // JSON logs, so the cluster checks below can read each worker's pid.
    { env: { ...process.env, RUVYXA_LOG_FORMAT: 'json' }, stdio: ['ignore', 'pipe', 'pipe'] },
  )
  child.stdout.on('data', (chunk) => (output += chunk))
  child.stderr.on('data', (chunk) => (output += chunk))
}
```

- In the dispatch block (`if (runtime === 'static') …`), add
  `else if (runtime === 'start') startRuvyxaStart()` before the final `else`.
- `publishDirectory()` (line 404): for `start` return
  `path.join(deploymentDirectory, '.ruvyxa', 'deploy', 'node', 'public')`.
- Everywhere the script branches on `runtime === 'node'` or `NODE_PROGRAM.has(runtime)` for
  expectations about the emitted server (not for how to spawn it), treat `start` as `node`. Find
  them with `rg -n "runtime === 'node'|NODE_PROGRAM.has" scripts/smoke-runtime-adapter.mjs`; the one
  at line 210 is spawn-only and stays as is.

- [ ] **Step 2: Add the cluster checks** to the end of the check sequence, guarded by
      `if (runtime === 'start')`:

```js
if (runtime === 'start') {
  // Every worker answers. Responses from a cluster carry no worker id, so the
  // check is that the supervisor reported the count and that a burst of
  // concurrent requests all succeed.
  assert(output.includes('supervising workers'), 'start did not supervise workers', output)
  const burst = await Promise.all(
    Array.from({ length: 20 }, () => fetch(`${base}/__ruvyxa/health`)),
  )
  assert(
    burst.every((response) => response.status === 200),
    'a worker failed a health probe',
  )

  // A killed worker is replaced.
  const before = (output.match(/worker exited, respawning/g) ?? []).length
  const pids = [...output.matchAll(/"pid":(\d+)/g)].map((match) => Number(match[1]))
  if (pids.length > 0) {
    process.kill(pids[0], 'SIGKILL')
    await waitFor(() => (output.match(/worker exited, respawning/g) ?? []).length > before, 10_000)
  }

  // A drain reaches every worker: health reads `draining` before the socket closes.
  child.kill('SIGTERM')
  const draining = await fetch(`${base}/__ruvyxa/health`).then((response) => response.json())
  assert(draining.status === 'draining', 'health did not report draining after SIGTERM')
}
```

Use the script's existing assertion and wait helpers — read their names in the file (they may be
called differently from `assert`/`waitFor`; use whatever it defines). The pid check needs each
worker's `listening on` log line to include `pid: process.pid`; add `pid: process.pid` to the three
`listening on` log calls in `standalone-server.ts` (Task 4's file) and re-run Task 4's tests. On
Windows `SIGTERM` via `child.kill` is a hard kill, so guard the drain assertion with
`process.platform !== 'win32'`, matching how `scripts/` already handle Windows drain tests.

- [ ] **Step 3: Run the lane locally.**

```bash
cargo build -p ruvyxa_cli
cargo run -p ruvyxa_cli -- build --root examples/deploy-smoke
RUVYXA_CLI=target/debug/ruvyxa node scripts/smoke-runtime-adapter.mjs start examples/deploy-smoke 4402
```

(on Windows: `$env:RUVYXA_CLI='target\debug\ruvyxa.exe'`). Expected: every check passes. Then with
Bun (`--runtime bun` on build) and Deno (`--runtime deno`, with `RUVYXA_SMOKE_WORKERS=1` — Deno
refuses more than one) using `~/.bun/bin` and `~/.deno/bin` binaries.

- [ ] **Step 4: Prove the Deno refusal.** `RUVYXA_SMOKE_WORKERS=2` with a deno build — Expected: the
      lane fails at startup with output containing `RUV1025` and `not supported on Deno`. This is a
      manual negative check; record the output in the report.

- [ ] **Step 5: Add the CI step** after "Build and launch Node deployment artifact":

```yaml
- name: Launch the build through ruvyxa start
  timeout-minutes: 15
  shell: bash
  run: |
    cargo build -p ruvyxa_cli
    cargo run -p ruvyxa_cli -- build --root examples/deploy-smoke
    RUVYXA_CLI="$PWD/target/debug/ruvyxa" node scripts/smoke-runtime-adapter.mjs start examples/deploy-smoke 4402
```

Then `cargo test -p ruvyxa_cli --test ci_workflows` — it asserts properties of every workflow step
(timeouts, pinning). Expected: pass.

- [ ] **Step 6: Checkpoint** — report diff and lane output. Do not commit.

---

### Task 8: Phase-2 retirement list and documentation

**Files:**

- Modify: `docs/superpowers/specs/2026-10-12-single-production-host-design.md` (Fixtures list)
- Modify: `docs/en/10-cli.md`, `docs/th/10-cli.md`
- Modify: `docs/en/15-deploy-run-and-operate.md`, `docs/th/15-deploy-run-and-operate.md`
- Modify: `docs/en/16-troubleshooting-upgrades.md`, `docs/th/16-troubleshooting-upgrades.md`
- Modify: `docs/en/09-integrations-auth-data-and-realtime.md`, `docs/th/09-…` (experimental note)
- Modify: `ARCHITECTURE.md`, `AGENTS.md`, `CLAUDE.md` ("Two request hosts" row), `CHANGELOG.md`

- [ ] **Step 1: Build the retirement list.** For each file in `tests/fixtures/`, list its readers:

```bash
rg -l --glob '!tests/fixtures/**' "<fixture-file-name>" crates packages tests scripts
```

A fixture goes on the list when it has a Rust reader in `crates/ruvyxa_dev_server` or
`crates/ruvyxa_middleware` **and** a JavaScript reader that exercises `serverless-handler.mjs`,
`@ruvyxa/core`, or the standalone server — i.e. it exists to hold the Axum host and `createHandler`
together. Write the list as a table (`fixture | Rust reader | JS reader`) under the spec's
"Fixtures" section. Fixtures held between the two compilers belong to Phase 3 and are listed
separately under a "Phase 3" sub-heading.

- [ ] **Step 2: CLI chapter (en, then th with the same structure).** In `10-cli.md`, the `start` row
      and its section: `start` runs the standalone server the build leaves under
      `<outDir>/deploy/<runtime>/`; flags `--workers <n|auto>` and `--legacy-host` (deprecated, one
      minor release); `ruvyxa build` must run first, and a rebuilt project needs a fresh `build`
      before `start` (RUV1022).

- [ ] **Step 3: Deploy chapter.** In `15-deploy-run-and-operate.md`: `RUVYXA_WORKERS` (also read by
      `node server/index.mjs` directly); per-worker state — ISR store, rate limiters, and in-memory
      caches are per process; use `cache.handler` for state shared across workers; Bun needs Linux
      for more than one worker; Deno runs one process per instance; `image.onDemand` works under
      `ruvyxa start` through its image service and answers 404 when the standalone server is started
      directly.

- [ ] **Step 4: Troubleshooting.** Add rows for `RUV1022`–`RUV1025` to the code table in
      `16-troubleshooting-upgrades.md`, matching the existing column format (code | cause | fix).

- [ ] **Step 5: Integrations chapter.** One sentence at the top of the auth/data/realtime chapter:
      those packages are experimental; plus realtime under `start` needs `--legacy-host` (RUV1024).

- [ ] **Step 6: Architecture notes.** `ARCHITECTURE.md` "System Boundaries" and "Deployment
      Adapters": production has one request host; Axum serves `dev` and `--legacy-host` until
      Phase 2. `AGENTS.md` and `CLAUDE.md` "Two request hosts" row: "Production has one host
      (`createHandler`); `dev` still runs on Axum until Phase 2, so a feature still has to work in
      both for now." `CHANGELOG.md` "Unreleased": the `start` change, `--workers`, `--legacy-host`,
      the experimental tier, the four codes, and the throughput note (set `--workers` where Axum's
      worker pool was relied on).

- [ ] **Step 7: Gates.**

```bash
node scripts/check-doc-links.mjs
node scripts/check-source-path-refs.mjs
pnpm format:check
```

Expected: all pass. A Thai anchor with combining marks can pass `check-doc-links` and still break on
GitHub — link to headings by their English chapter anchor where one exists.

- [ ] **Step 8: Checkpoint** — report diff. Do not commit.

---

### Task 9: Full battery and report

- [ ] **Step 1: Run all eleven checks** from Global Constraints, in order.
- [ ] **Step 2: Run the `start` lane** on Node, Bun, and Deno (Task 7 Step 3).
- [ ] **Step 3: Run the existing node, bun, deno adapter lanes** (`ci.yml` lines 190–227 commands)
      to prove the generated-server changes did not regress a deployment started without `start`.
- [ ] **Step 4: Report** to the owner: what changed (by task), every command run with its result,
      the manual checks from Tasks 6 and 7, anything skipped and why, and the Phase-2 retirement
      list. Do not commit.

---

## Self-review notes

- Spec coverage: Phase 0 baseline (Task 1), experimental tier + gate (Task 2), local standalone
  stage (Task 5), `start` launcher with errors and rollback (Task 6), `--workers` on all three
  runtimes (Task 4, verified in Task 7), drain and crash loop (Task 4, Task 7), image sidecar (Tasks
  3, 4, 6), testing items 1–5 (Tasks 4–7), fixtures list (Task 8), docs (Task 8), definition of done
  (Task 9).
- The crash-loop rule is unit-tested (Task 4); the end-to-end lane checks respawn after one kill but
  does not drive six crashes — record that limit in the Task 9 report.
- `image.onDemand` under `start` is proven by the Rust service tests (Task 3, a real PNG resized to
  WebP) and the emitted-source test (Task 4), not by an HTTP lane: `examples/deploy-smoke` does not
  turn the option on. Record that limit in the Task 9 report; adding an image route to the smoke
  fixture is a follow-up the owner can choose.
- The spec amendments in Task 0 are the two planning decisions (standalone location, image sidecar)
  and the fixture-list form; nothing else in the approved design changed.
