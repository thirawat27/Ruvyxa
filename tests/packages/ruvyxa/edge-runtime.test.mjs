/**
 * The JavaScript half of `tests/fixtures/edge-runtime-conformance.json`.
 *
 * Rust decides a route's runtime and writes it into the `deploy` section of the
 * build's `manifest.json`; every adapter reads it from there through the types
 * in `@ruvyxa/core`. A value one side can produce and the other cannot name is
 * a route placed somewhere nobody asked for, and nothing between them would
 * report it.
 *
 * The Rust half is `route_runtime_declarations_match_the_shared_conformance_table`
 * and `edge_unavailable_builtins_match_the_shared_conformance_table` in
 * `crates/ruvyxa_graph/src/tests.rs`; the list itself is `EDGE_UNAVAILABLE_BUILTINS`
 * in `crates/ruvyxa_graph/src/render.rs`.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const read = (relative) => readFileSync(path.join(workspaceRoot, relative), 'utf8')

const fixture = JSON.parse(read('tests/fixtures/edge-runtime-conformance.json'))
const adapters = JSON.parse(read('tests/fixtures/adapter-contract.json'))
const deployManifestSource = read('packages/@ruvyxa/core/src/deploy-manifest.ts')

/** The `runtime:` union the deploy manifest type declares, as a set. */
function declaredRuntimes() {
  const line = deployManifestSource.match(/^\s*runtime:\s*(.+)$/m)
  assert.ok(line, 'deploy-manifest.ts must declare a `runtime` field')
  return new Set([...line[1].matchAll(/'([^']+)'/g)].map((m) => m[1]))
}

describe('per-route edge runtime', () => {
  it('names only runtimes the deploy manifest type accepts', () => {
    const accepted = declaredRuntimes()
    const produced = new Set(fixture.declaration.values.map((testCase) => testCase.runtime))
    assert.ok(produced.size > 1, 'the table must exercise more than one runtime')
    for (const runtime of produced) {
      assert.ok(
        accepted.has(runtime),
        `the route walk can produce \`${runtime}\` and the manifest type cannot name it — ` +
          `accepted: ${[...accepted].join(', ')}`,
      )
    }
    assert.equal(fixture.declaration.default, 'node')
  })

  it('has at least one adapter that can place work on an edge runtime', () => {
    // Without one the declaration could never be honoured by anything, and the
    // build-time constraint would be the whole feature.
    const edgeCapable = adapters.adapters.filter((adapter) => adapter.runtime === 'edge')
    assert.ok(edgeCapable.length > 0, 'no adapter declares an edge runtime')
    for (const adapter of edgeCapable) {
      assert.equal(
        adapter.target,
        'edge',
        `${adapter.name} claims an edge runtime on a ${adapter.target} target`,
      )
    }
  })

  it('keeps the two built-in lists disjoint and sorted', () => {
    const unavailable = fixture.unavailableOnEdge
    const available = fixture.availableOnEdge
    assert.deepEqual(unavailable, [...unavailable].sort(), 'unavailableOnEdge must stay sorted')
    assert.deepEqual(available, [...available].sort(), 'availableOnEdge must stay sorted')
    const overlap = unavailable.filter((name) => available.includes(name))
    assert.deepEqual(
      overlap,
      [],
      `a module cannot be both refused and allowed on edge: ${overlap.join(', ')}`,
    )
  })

  it('refuses only modules that genuinely need a host', () => {
    // A regression here is silent in the expensive direction: adding a widely
    // polyfilled module to the refused list fails builds that would have run.
    for (const polyfilled of ['buffer', 'crypto', 'path', 'stream', 'url', 'util']) {
      assert.ok(
        !fixture.unavailableOnEdge.includes(polyfilled),
        `${polyfilled} is polyfilled by every edge runtime and must not be refused`,
      )
    }
    for (const hostBound of ['fs', 'child_process', 'net', 'worker_threads']) {
      assert.ok(
        fixture.unavailableOnEdge.includes(hostBound),
        `${hostBound} needs a host no V8 isolate has and must be refused`,
      )
    }
  })
})
