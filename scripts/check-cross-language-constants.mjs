#!/usr/bin/env node
// Ruvyxa's compiler and server are Rust and its runtime is TypeScript, so a
// rule that both halves need is written down twice. Every expensive defect this
// repository has shipped has that shape, and the cheapest of them are the ones
// where the duplicated thing is a single literal:
//
//   defaultMaxWidth   3840 lived in a fixture with a Rust replay, and in three
//                     more JavaScript literals written into deployed functions
//                     that nothing gated.
//   STATIC_CONTENT_TYPES / DEFAULT_SECURITY_HEADERS
//                     kept in step by a comment saying they mirrored each other,
//                     until they did not.
//
// Neither drift fails a build. The two halves serve the same application from
// different hosts, so the split shows up as a page behaving one way under
// `ruvyxa dev` and another way once deployed -- which is the most expensive
// place to find it.
//
// This check enumerates every SCREAMING_SNAKE constant declared in *both*
// languages and requires each name to say, here, what holds the two together.
// It deliberately does not diff values across the board: the same fact is
// legitimately encoded differently in the two languages (`&["tsx"]` against
// `['.tsx']`, `52_428_800` against `50 * 1024 * 1024`), so a blanket comparison
// would be noise rather than a gate. Instead every pair is registered as one of
// four kinds, and three of them are enforced:
//
//   fixture     a shared conformance table both languages replay. The file must
//               exist.
//   test        a cross-language test that reads both sources. The file must
//               exist.
//   sameValue   a scalar this script normalizes and compares itself. This is a
//               real gate: change 10_000 on one side and it fails here.
//   unrelated   the same name meaning two different things. Needs a reason.
//
// A name that is declared in both languages and registered nowhere fails. So
// does a registry entry whose name is no longer declared in both -- a reason
// nothing stands behind is how a list like this rots.
//
// A "pair" is the wrong picture, and believing it cost this check most of its
// reach for a while. The repository's real shape is often three copies -- the
// Rust writer, the typed reader in `@ruvyxa/core`, and the executed reader in
// `packages/ruvyxa/runtime` that a deployed build actually runs -- so every
// declaration of a name is collected, and a `sameValue` name must agree across
// all of them. It used to keep only the first file `git ls-files` handed it,
// which sorts `packages/@ruvyxa/` before `packages/ruvyxa/`, so the copy that
// decides whether a deploy is accepted was the one copy never compared.
//
// What this does not catch, said plainly so it is not mistaken for more than it
// is: a pair that shares a fact but not a name, and a constant on one side
// facing a bare inline literal on the other. `defaultMaxWidth` was the second
// shape -- one Rust constant against three unnamed `3840`s written into
// deployed functions -- and no name-matching check would have seen it. Naming
// both halves the same thing is what brings a pair into this gate's reach, and
// is the cheapest reason to do it.
import { readFileSync, existsSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * This repository's root, from this file's own location rather than from
 * `process.cwd()`. The paths this gate reports are repository-relative because
 * that is how they read in a failure message; they are resolved against this
 * before anything opens them.
 */
const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

/** Anything shorter reads as an abbreviation and collides by accident. */
const MIN_NAME_LENGTH = 4

export const RUST_CONST =
  /^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?const[ \t]+([A-Z][A-Z0-9_]+)[ \t]*:[^=]+=([\s\S]*?);[ \t]*$/gm
export const JS_CONST = /^[ \t]*(?:export[ \t]+)?const[ \t]+([A-Z][A-Z0-9_]+)[ \t]*=[ \t]*(.*)$/gm

// Each entry: the shared name, the kind that holds it, and why. `held` names the
// fixture or test file for those kinds.
export const REGISTRY = [
  {
    name: 'BOOTSTRAP_ELEMENT_ID',
    kind: 'fixture',
    held: 'tests/fixtures/client-bootstrap-conformance.json',
    why: 'The element the client entry reads its route context out of; the document writer and the entry template must name the same one.',
  },
  {
    name: 'CLIENT_BUILD_REPORT_FILE',
    kind: 'sameValue',
    why: 'The build report the pre-renderer and every deployed function read each route’s browser assets out of. The build writes it; the adapter runner reads it back. A split silently loads nothing, and every deployed page then ships without its script tag.',
  },
  {
    name: 'COMPONENT_EXTENSIONS',
    kind: 'fixture',
    held: 'tests/fixtures/route-chain-conformance.json',
    why: 'The extensions a route component may be written in, and the order they are probed in. Both graphs walk the same `app/` tree: a rule taught to one leaves a project written in the other extension losing every layout and template in that host alone — no diagnostic, a successful build, and a page rendered without its `<html>`/`<body>` shell. The order matters too, because it decides which file a project holding both composes.',
  },
  {
    name: 'DEFAULT_REVALIDATE_SECONDS',
    kind: 'sameValue',
    why: 'The ISR window a route gets when it names none. A split makes the same route revalidate on two schedules depending on where it is served.',
  },
  {
    name: 'DEFAULT_WORKER_SHUTDOWN_GRACE_MS',
    kind: 'sameValue',
    why: 'How long a render worker may keep running after its stdin closes. The worker spends the window; the host enforces it and kills the process once its own ceiling — this value plus a margin — passes. They were written independently, 5 s against a 2 s host wait, so the grace was unreachable and every shutdown with a request in flight was a kill. A split here does not fail anything; it just silently removes the only way an in-flight request survives a worker replacement.',
  },
  {
    name: 'DEFAULT_SECURITY_HEADERS',
    kind: 'fixture',
    held: 'tests/fixtures/security-headers-conformance.json',
    why: 'A structured table, and one that has drifted before.',
  },
  {
    name: 'DEPLOY_MANIFEST_KEY',
    kind: 'sameValue',
    why: 'The route-manifest key the build writes the deploy manifest under and the adapter runner reads it back from. A split reads nothing and reports no error.',
  },
  {
    name: 'DEPLOY_MANIFEST_VERSION',
    kind: 'sameValue',
    why: 'The contract version the writer stamps and the reader refuses to exceed. If the writer moves alone every deployed build is refused; if the reader moves alone it parses a shape it does not know.',
  },
  {
    name: 'DOCUMENT_CACHE_CONTROL',
    kind: 'fixture',
    held: 'tests/fixtures/deploy-output-conformance.json',
    why: 'Replayed by both hosts as part of the document cache-control table.',
  },
  {
    name: 'DOCUMENT_VALIDATOR_STRATEGIES',
    kind: 'fixture',
    held: 'tests/fixtures/deploy-output-conformance.json',
    why: 'Which documents carry a validator, replayed by both. The value is deliberately host-local — blake3 here, SHA-256 there — but the membership is not: a host that validated an `ssr` document would answer 304 for a page rendered for somebody else.',
  },
  {
    name: 'HELPER_RUNTIME_PREFIX',
    kind: 'sameValue',
    why: 'The specifier prefix both resolvers recognise as an oxc helper import. Teaching one and not the other is the two-module-graphs trap in its cheapest form.',
  },
  {
    name: 'IMMUTABLE_CACHE_CONTROL',
    kind: 'sameValue',
    why: 'How long a hashed client bundle may be reused. `ruvyxa start` sends it and the emitted standalone server sends it for the same file, so a project that disagreed would ship one lifetime in development and another in production.',
  },
  {
    name: 'INSTRUMENTATION_FILES',
    kind: 'fixture',
    held: 'tests/fixtures/instrumentation-files-conformance.json',
    why: 'A list, replayed by both.',
  },
  {
    name: 'ISR_EXPIRE_SECONDS',
    kind: 'fixture',
    held: 'tests/fixtures/deploy-output-conformance.json',
    why: 'Part of the emitted cache-control table both hosts replay.',
  },
  {
    name: 'KNOWN_ADAPTER_NAMES',
    kind: 'test',
    held: 'tests/packages/ruvyxa/known-adapter-names.test.mjs',
    why: 'The CLI validates `--adapter <name>` against one list and the adapter runner resolves against the other, so a name in one and not the other is either a flag that builds nothing or an adapter the flag refuses.',
  },
  {
    name: 'MARKER',
    kind: 'unrelated',
    why: 'A local scratch name, not one fact: `compiler.rs` marks `import.meta.env` and `glob.mjs` marks `import.meta.glob`. Nothing is meant to agree.',
  },
  {
    name: 'MAX_IMAGE_QUALITY',
    kind: 'fixture',
    held: 'tests/fixtures/dynamic-image-conformance.json',
    why: 'The top of the quality scale, and one number in three places: the build encodes with it, the deployed handler clamps a request to it, and the CLI now refuses a configured value above it. The fixture already declared the bound for the two request hosts; the config validator replays the same entry rather than restating the number.',
  },
  {
    name: 'MAX_NODE_TIMEOUT_MS',
    kind: 'sameValue',
    why: "Node's own ceiling on a timer delay (2^31-1), not a Ruvyxa policy — but both halves schedule against it, and a host that clamped differently would silently fire immediately.",
  },
  {
    name: 'MAX_TRACKED_RATE_LIMIT_KEYS',
    kind: 'sameValue',
    why: 'The bound that keeps a rate limiter from being the memory-exhaustion vector it exists to prevent. A split gives one host a different resistance from the other under the same configuration.',
  },
  {
    name: 'MDX_COMPONENT_EXTENSIONS',
    kind: 'fixture',
    held: 'tests/fixtures/mdx-components-conformance.json',
    why: 'A list, replayed by both.',
  },
  {
    name: 'MODULE_KIND_EXTENSIONS',
    kind: 'fixture',
    held: 'tests/fixtures/module-kind-conformance.json',
    why: 'A list, replayed by both.',
  },
  {
    name: 'NOT_FOUND_DOCUMENT_FILE',
    kind: 'sameValue',
    why: 'The file the prerenderer writes the 404 document to and the adapter runner looks for. A split serves no 404 page at all.',
  },
  {
    name: 'PUBLIC_ASSET_CACHE_CONTROL',
    kind: 'sameValue',
    why: 'The same fact for an unhashed `public/` asset. Both hosts also restate it on the 304 that answers its revalidation, so the value is read three times and must be one.',
  },
  {
    name: 'ROUTE_SLOT_PRELUDE',
    kind: 'fixture',
    held: 'tests/fixtures/entry-composition-conformance.json',
    why: 'An entire emitted function body written twice. The two entry composers must emit the same runtime behaviour for parallel routes.',
  },
  {
    name: 'RSC_PAYLOAD_ELEMENT_ID',
    kind: 'sameValue',
    why: 'The element the document writer puts the inline Flight payload in and the client runtime reads it out of. A split hydrates from nothing.',
  },
  {
    name: 'RSC_REQUEST_HEADER',
    kind: 'fixture',
    held: 'tests/fixtures/framework-endpoint-conformance.json',
    why: 'Part of the endpoint conformance table both hosts replay.',
  },
  {
    name: 'SERVER_ACTION_HEADER',
    kind: 'fixture',
    held: 'tests/fixtures/framework-endpoint-conformance.json',
    why: 'Part of the endpoint conformance table both hosts replay.',
  },
  {
    name: 'SITEMAP_FOOTER',
    kind: 'sameValue',
    why: 'Two sitemap writers close the document; one that closed it differently would emit a sitemap the other language considers malformed.',
  },
  {
    name: 'SITEMAP_MAX_BYTES',
    kind: 'sameValue',
    why: "The sitemap protocol's per-document byte ceiling. A split writes a document one writer considers valid and the protocol does not.",
  },
  {
    name: 'SITEMAP_MAX_LOCATION_CHARS',
    kind: 'sameValue',
    why: 'The protocol ceiling on one `<loc>`. A split drops a URL in one writer and keeps it in the other.',
  },
  {
    name: 'SITEMAP_MAX_URLS',
    kind: 'sameValue',
    why: 'The protocol ceiling on entries per document, and what decides where a sitemap is split into several.',
  },
  {
    name: 'STATIC_ASSET_EXTENSIONS',
    kind: 'fixture',
    held: 'tests/fixtures/static-asset-conformance.json',
    why: 'A list, replayed by both.',
  },
  {
    name: 'STATIC_PARAMS_EXPORTS',
    kind: 'test',
    held: 'tests/packages/ruvyxa/static-params-names.test.mjs',
    why: 'That test reads both sources and compares the lists in order; a name recognised by the graph and not the worker discovers as SSG and pre-renders nothing.',
  },
  {
    name: 'TELEMETRY_FIELDS',
    kind: 'unrelated',
    why: 'Two different removal strategies for two different comparisons, not one list said twice. `bench.rs` normalizes the client build report — `client-report.json` at the build root — across a cold and a warm build and drops the whole `cache` object, so the counters inside it — `graphHits`, `hits`, `misses` — never need naming; it does not compare `build.json`, so `createdAtUnix` is not its problem either. `verify-reproducible.mjs` compares two cold builds across every emitted JSON file, keeps the `cache` object and names its counters one by one, adds `createdAtUnix` because `build.json` is in its comparison, and matches every `*Ms` key by shape so a new timing phase does not silently start failing it. Forcing the two equal would make each list carry entries its own comparison can never see, which is how a list stops meaning anything.',
  },
]

// The pathspec that decides what this gate can see.
//
// The scripts entry is `scripts/*.mjs`, and the spelling matters: `git
// ls-files` matches with a plain `fnmatch` where `*` crosses `/`, so
// `packages/**` and `packages/*` select the same 48 files — while the
// double-star spelling of the scripts entry selects *zero*, because the slash
// after it is literal and every script sits one level down. A pathspec that
// quietly matches nothing is a gate that quietly checks nothing, and that is
// what kept `verify-reproducible.mjs`'s `TELEMETRY_FIELDS` out of reach of the
// one check written to find exactly that shape. Its own test asserts on the
// files this selects, not on the pattern, for the same reason.
//
// `scripts/` belongs here because the repository's own gates are two-language
// facts too: a checker that mirrors a Rust list is the same duplication as a
// runtime that mirrors one, and it rots the same way.
export const SOURCE_PATHSPEC = [
  'crates/**/*.rs',
  'packages/**/*.mjs',
  'packages/**/*.ts',
  'scripts/*.mjs',
]

/**
 * Every tracked source file of either language, in `git ls-files` order.
 *
 * Run from `REPO_ROOT`, not from wherever the caller happens to be. A git
 * pathspec is resolved against the working directory, so `crates/**` asked from
 * inside a package directory names `<package>/crates/**` and matches nothing —
 * and this gate answers a question whose correct answer is "nothing to report"
 * either way, so it passed. `pnpm -r test` runs each package's script from that
 * package's own directory, which is exactly the caller it was silent for.
 */
export function trackedSources() {
  return execFileSync('git', ['ls-files', ...SOURCE_PATHSPEC], {
    encoding: 'utf8',
    cwd: REPO_ROOT,
  })
    .split('\n')
    .filter(Boolean)
    .filter((file) => !file.includes('/dist/') && !file.endsWith('.d.ts'))
}

/**
 * Every declaration, by name — `Map<string, Array<{ file, value }>>`.
 *
 * Every copy is kept, not the first one seen. Keeping one meant the gate read
 * the order `git ls-files` happened to emit as a statement about which copy
 * mattered, and `packages/ruvyxa/runtime/` sorts last: the deployed reader was
 * the copy never compared. Repetition inside one language is not noise to be
 * suppressed here — it is either the same fact said twice, which the comparison
 * folds away, or a divergence, which is the whole point.
 *
 * Rust test modules are cut away: a constant that exists only to drive a test
 * is not a rule the other language answers, and counting it would push the
 * registry toward the wall of excuses this check is meant to avoid.
 */
export function declarations(files, pattern, cutAtTestModule) {
  const found = new Map()
  for (const file of files) {
    // Line endings are whatever the checkout produced, and the patterns anchor
    // at end of line. A `\r` left in place makes this check quietly see half
    // the declarations on Windows and all of them on CI, which is the exact
    // failure mode it exists to catch.
    // Resolved against the repository root, which leaves an absolute path — a
    // test's scratch file — exactly as it was given.
    let source = readFileSync(path.resolve(REPO_ROOT, file), 'utf8').replace(/\r\n/g, '\n')
    if (cutAtTestModule) {
      // The test *module*, not every `#[cfg(test)]`. That attribute also marks
      // individual test-only helpers, and several files carry one hundreds of
      // lines above their production constants — cutting at the first one hid
      // six real declarations from this check the first time it ran.
      const testModule = source.search(/^#\[cfg\(test\)\]\nmod\b/m)
      if (testModule !== -1) source = source.slice(0, testModule)
    }
    for (const match of source.matchAll(pattern)) {
      const [, name, value] = match
      if (name.length < MIN_NAME_LENGTH) continue
      const copies = found.get(name)
      if (copies) copies.push({ file, value: value.trim() })
      else found.set(name, [{ file, value: value.trim() }])
    }
  }
  return found
}

/**
 * A scalar literal as a comparable string, or `null` when it is not one.
 *
 * Numeric separators, Rust's raw and typed literals, and an arithmetic product
 * of plain numbers all say the same thing in two spellings, so they are folded.
 * Anything with an identifier in it is refused rather than guessed at: the
 * point of `sameValue` is that a mismatch means something, which requires the
 * match to mean something too.
 */
export function scalar(raw) {
  let text = raw.trim().replace(/\s+as\s+const$/, '')
  const string = text.match(/^r?#*"([\s\S]*)"#*$/) ?? text.match(/^'([\s\S]*)'$/)
  if (string) return `string:${string[1]}`
  text = text.replace(/_/g, '')
  if (/^-?\d+(?:\.\d+)?(?:_?[iuf](?:8|16|32|64|size))?$/.test(text)) {
    return `number:${Number.parseFloat(text)}`
  }
  // A pure arithmetic spelling of a number, such as `50 * 1024 * 1024`.
  //
  // The guard keeps every identifier out of `Function()`, so nothing here can
  // execute code from a source file -- but it does not make the expression
  // *complete*. `JS_CONST` captures an initializer to the end of its line, so a
  // constant whose arithmetic wraps across two lines arrives as `(50 *`, which
  // satisfies the character guard and throws a `SyntaxError` out of `Function`.
  // That reached the user as a Node stack trace from `pnpm release:validate`
  // naming neither the constant nor the file it came from.
  //
  // `null` is the right answer for it, not a crash: it routes into the existing
  // "no longer a scalar this check can compare" failure below, which names both.
  if (/^[\d\s*+()]+$/.test(text) && /\d/.test(text)) {
    try {
      const product = Function(`"use strict"; return (${text})`)()
      if (Number.isFinite(product)) return `number:${product}`
    } catch {
      return null
    }
  }
  if (text === 'true' || text === 'false') return `boolean:${text}`
  return null
}

/**
 * Every place a name is declared, one line each.
 *
 * The whole list, always. When three files carry a name and one of them
 * drifted, naming two of them tells the reader nothing about which file to
 * open — and the file left out is the one this check used to be blind to.
 */
function locations(inRust, inJs, describe) {
  return [
    ...inRust.map((copy) => `      rust: ${describe(copy)}`),
    ...inJs.map((copy) => `      js:   ${describe(copy)}`),
  ].join('\n')
}

/** Which side still has the name, for a registry entry that has stopped applying. */
function survivor(rust, js, name) {
  if (rust.has(name)) return 'Only Rust'
  if (js.has(name)) return 'Only JavaScript'
  return 'Neither language'
}

/**
 * Judge two declaration maps against the registry.
 *
 * `compared` is the number of declarations this pass actually had in hand for
 * shared names — reported on success, because the count is what shows that the
 * extra copies are in reach. It read `2 × names` for as long as the collection
 * kept one copy per language, and nothing said so.
 */
export function inspect(rust, js, registry = REGISTRY) {
  const failures = []
  const registered = new Map(registry.map((entry) => [entry.name, entry]))
  const shared = [...rust.keys()].filter((name) => js.has(name)).sort()
  let compared = 0

  for (const name of shared) {
    const entry = registered.get(name)
    const inRust = rust.get(name)
    const inJs = js.get(name)
    compared += inRust.length + inJs.length
    if (!entry) {
      failures.push(
        `${name} is declared in both languages and registered nowhere.\n` +
          `${locations(inRust, inJs, (copy) => copy.file)}\n` +
          '      Add it to REGISTRY in this file: a shared fixture both languages replay, a\n' +
          '      cross-language test, `sameValue` if it is a scalar this script can compare,\n' +
          '      or `unrelated` if the two names mean different things.',
      )
      continue
    }
    if (entry.kind === 'fixture' || entry.kind === 'test') {
      if (!existsSync(path.resolve(REPO_ROOT, entry.held))) {
        failures.push(
          `${name} is registered as held by ${entry.held}, which does not exist.\n` +
            '      Point the entry at what actually holds the pair, or change its kind.',
        )
      }
      continue
    }
    if (entry.kind !== 'sameValue') continue

    // De-duplication belongs here and not in collection: two files saying the
    // same thing are one fact and fold away, and a set with more than one
    // member is the split this check exists to fail on. Dropping the repeat
    // earlier decides which copy counts before anything has looked at it.
    const copies = [...inRust, ...inJs].map((copy) => ({ ...copy, normalized: scalar(copy.value) }))
    if (copies.some((copy) => copy.normalized === null)) {
      failures.push(
        `${name} is registered as \`sameValue\` but is no longer a scalar this check can compare.\n` +
          `${locations(inRust, inJs, (copy) => `${copy.file}: ${copy.value.slice(0, 60)}`)}\n` +
          '      Give the name a shared fixture and register it as one; an uncomparable\n' +
          '      `sameValue` entry is a gate that has stopped gating.',
      )
      continue
    }
    if (new Set(copies.map((copy) => copy.normalized)).size > 1) {
      failures.push(
        `${name} says more than one thing.\n` +
          `${locations(inRust, inJs, (copy) => `${copy.file}: ${copy.value}`)}\n` +
          `      ${entry.why}`,
      )
    }
  }

  for (const entry of registry) {
    if (shared.includes(entry.name)) continue
    failures.push(
      `${entry.name} is registered here but is no longer declared in both languages.\n` +
        `      ${survivor(rust, js, entry.name)} declares it now.\n` +
        '      Remove the entry; a reason nothing stands behind is how this list rots.',
    )
  }

  return { failures, shared, compared }
}

function main() {
  const tracked = trackedSources()
  const rust = declarations(
    tracked.filter((file) => file.endsWith('.rs')),
    RUST_CONST,
    true,
  )
  const js = declarations(
    tracked.filter((file) => !file.endsWith('.rs')),
    JS_CONST,
    false,
  )
  const { failures, shared, compared } = inspect(rust, js)

  if (failures.length > 0) {
    console.error('Cross-language constants are not held together:\n')
    for (const failure of failures) console.error(`  ${failure}\n`)
    console.error(
      `${failures.length} problem${failures.length === 1 ? '' : 's'}.\n` +
        'One rule written in two languages needs something that fails when they disagree.\n' +
        'A comment saying they mirror each other is not that thing.',
    )
    process.exitCode = 1
  } else {
    console.log(
      `Checked ${tracked.length} source files; ${shared.length} constants are declared in both ` +
        `languages across ${compared} declarations, and all ${REGISTRY.length} are held.`,
    )
  }
}

// Running the file checks the repository; importing it hands the pieces to a
// test that drives them over sources it controls. This check shipped with no
// test of its own, and the defect that cost it most of its reach — keeping one
// copy per name — is the kind a unit test states in a line.
if (process.argv[1] !== undefined && path.resolve(process.argv[1]) === import.meta.filename) {
  main()
}
