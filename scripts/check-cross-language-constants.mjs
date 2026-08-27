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
// What this does not catch, said plainly so it is not mistaken for more than it
// is: a pair that shares a fact but not a name, and a constant on one side
// facing a bare inline literal on the other. `defaultMaxWidth` was the second
// shape -- one Rust constant against three unnamed `3840`s written into
// deployed functions -- and no name-matching check would have seen it. Naming
// both halves the same thing is what brings a pair into this gate's reach, and
// is the cheapest reason to do it.
import { readFileSync, existsSync } from 'node:fs'
import { execFileSync } from 'node:child_process'

/** Anything shorter reads as an abbreviation and collides by accident. */
const MIN_NAME_LENGTH = 4

const RUST_CONST =
  /^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?const[ \t]+([A-Z][A-Z0-9_]+)[ \t]*:[^=]+=([\s\S]*?);[ \t]*$/gm
const JS_CONST = /^[ \t]*(?:export[ \t]+)?const[ \t]+([A-Z][A-Z0-9_]+)[ \t]*=[ \t]*(.*)$/gm

// Each entry: the shared name, the kind that holds it, and why. `held` names the
// fixture or test file for those kinds.
const REGISTRY = [
  {
    name: 'BOOTSTRAP_ELEMENT_ID',
    kind: 'fixture',
    held: 'tests/fixtures/client-bootstrap-conformance.json',
    why: 'The element the client entry reads its route context out of; the document writer and the entry template must name the same one.',
  },
  {
    name: 'DEFAULT_REVALIDATE_SECONDS',
    kind: 'sameValue',
    why: 'The ISR window a route gets when it names none. A split makes the same route revalidate on two schedules depending on where it is served.',
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
    name: 'HELPER_RUNTIME_PREFIX',
    kind: 'sameValue',
    why: 'The specifier prefix both resolvers recognise as an oxc helper import. Teaching one and not the other is the two-module-graphs trap in its cheapest form.',
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
]

const tracked = execFileSync(
  'git',
  ['ls-files', 'crates/**/*.rs', 'packages/**/*.mjs', 'packages/**/*.ts'],
  { encoding: 'utf8' },
)
  .split('\n')
  .filter(Boolean)
  .filter((file) => !file.includes('/dist/') && !file.endsWith('.d.ts'))

/**
 * Declarations, by name.
 *
 * Rust test modules are cut away: a constant that exists only to drive a test
 * is not a rule the other language answers, and counting it would push the
 * registry toward the wall of excuses this check is meant to avoid.
 */
function declarations(files, pattern, cutAtTestModule) {
  const found = new Map()
  for (const file of files) {
    // Line endings are whatever the checkout produced, and the patterns anchor
    // at end of line. A `\r` left in place makes this check quietly see half
    // the declarations on Windows and all of them on CI, which is the exact
    // failure mode it exists to catch.
    let source = readFileSync(file, 'utf8').replace(/\r\n/g, '\n')
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
      if (name.length < MIN_NAME_LENGTH || found.has(name)) continue
      found.set(name, { file, value: value.trim() })
    }
  }
  return found
}

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

/**
 * A scalar literal as a comparable string, or `null` when it is not one.
 *
 * Numeric separators, Rust's raw and typed literals, and an arithmetic product
 * of plain numbers all say the same thing in two spellings, so they are folded.
 * Anything with an identifier in it is refused rather than guessed at: the
 * point of `sameValue` is that a mismatch means something, which requires the
 * match to mean something too.
 */
function scalar(raw) {
  let text = raw.trim().replace(/\s+as\s+const$/, '')
  const string = text.match(/^r?#*"([\s\S]*)"#*$/) ?? text.match(/^'([\s\S]*)'$/)
  if (string) return `string:${string[1]}`
  text = text.replace(/_/g, '')
  if (/^-?\d+(?:\.\d+)?(?:_?[iuf](?:8|16|32|64|size))?$/.test(text)) {
    return `number:${Number.parseFloat(text)}`
  }
  // A pure arithmetic spelling of a number, such as `50 * 1024 * 1024`.
  if (/^[\d\s*+()]+$/.test(text) && /\d/.test(text)) {
    const product = Function(`"use strict"; return (${text})`)()
    if (Number.isFinite(product)) return `number:${product}`
  }
  if (text === 'true' || text === 'false') return `boolean:${text}`
  return null
}

const failures = []
const registered = new Map(REGISTRY.map((entry) => [entry.name, entry]))
const shared = [...rust.keys()].filter((name) => js.has(name)).sort()

for (const name of shared) {
  const entry = registered.get(name)
  const inRust = rust.get(name)
  const inJs = js.get(name)
  if (!entry) {
    failures.push(
      `${name} is declared in both languages and registered nowhere.\n` +
        `      rust: ${inRust.file}\n` +
        `      js:   ${inJs.file}\n` +
        '      Add it to REGISTRY in this file: a shared fixture both languages replay, a\n' +
        '      cross-language test, `sameValue` if it is a scalar this script can compare,\n' +
        '      or `unrelated` if the two names mean different things.',
    )
    continue
  }
  if (entry.kind === 'fixture' || entry.kind === 'test') {
    if (!existsSync(entry.held)) {
      failures.push(
        `${name} is registered as held by ${entry.held}, which does not exist.\n` +
          '      Point the entry at what actually holds the pair, or change its kind.',
      )
    }
    continue
  }
  if (entry.kind !== 'sameValue') continue

  const rustValue = scalar(inRust.value)
  const jsValue = scalar(inJs.value)
  if (rustValue === null || jsValue === null) {
    failures.push(
      `${name} is registered as \`sameValue\` but is no longer a scalar this check can compare.\n` +
        `      rust: ${inRust.file}: ${inRust.value.slice(0, 60)}\n` +
        `      js:   ${inJs.file}: ${inJs.value.slice(0, 60)}\n` +
        '      Give the pair a shared fixture and register it as one; an uncomparable\n' +
        '      `sameValue` entry is a gate that has stopped gating.',
    )
    continue
  }
  if (rustValue !== jsValue) {
    failures.push(
      `${name} says two different things.\n` +
        `      rust: ${inRust.file}: ${inRust.value}\n` +
        `      js:   ${inJs.file}: ${inJs.value}\n` +
        `      ${entry.why}`,
    )
  }
}

/** Which side still has the name, for a registry entry that has stopped applying. */
function survivor(name) {
  if (rust.has(name)) return 'Only Rust'
  if (js.has(name)) return 'Only JavaScript'
  return 'Neither language'
}

for (const entry of REGISTRY) {
  if (shared.includes(entry.name)) continue
  failures.push(
    `${entry.name} is registered here but is no longer declared in both languages.\n` +
      `      ${survivor(entry.name)} declares it now.\n` +
      '      Remove the entry; a reason nothing stands behind is how this list rots.',
  )
}

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
    `Checked ${tracked.length} source files; ${shared.length} constants are declared in both languages and all ${REGISTRY.length} are held.`,
  )
}
