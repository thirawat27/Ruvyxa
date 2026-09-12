/**
 * The Flight decoder every browser runs, against the shared wire contract.
 *
 * `packages/ruvyxa/runtime/flight.mjs` writes every payload and decodes one
 * beside the encoder; this module decodes the ones that actually arrive. Only
 * the first had a test, so the round trip made the format look covered while
 * the reader that runs was exercised by nothing at all — and a divergence here
 * produces no error anybody sees: `startFlight` rejects, the router falls back
 * to `hardNavigate`, and a soft navigation becomes a full page load.
 *
 * `tests/fixtures/flight-conformance.json` is replayed by both.
 */

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { decodeFlight } from '../dist/router.js'

const repoRoot = path.resolve(fileURLToPath(new URL('../../../..', import.meta.url)))
const contract = JSON.parse(
  readFileSync(path.join(repoRoot, 'tests/fixtures/flight-conformance.json'), 'utf8'),
)
const routerSource = readFileSync(
  path.join(repoRoot, 'packages/@ruvyxa/react/src/router.ts'),
  'utf8',
)

const ARTIFACT_VERSION = '0123456789abcdef'
const ROUTE = '/guide'

/**
 * One payload, as the encoder writes it.
 *
 * Built here rather than imported from `flight.mjs` on purpose: the point of
 * the fixture is that these are two independent readers of one format, and a
 * test that got its bytes from the other implementation would agree with it by
 * construction.
 */
function envelope(tree, overrides = {}) {
  return JSON.stringify({
    protocol: contract.protocol,
    protocolVersion: contract.protocolVersion,
    manifestVersion: ARTIFACT_VERSION,
    route: ROUTE,
    tree,
    ...overrides,
  })
}

/** The shapes a JSON literal cannot express, one step past each bound. */
function build(name) {
  if (name === 'atDepth') {
    let tree = 'leaf'
    for (let level = 0; level < contract.limits.maxDepth; level += 1) tree = { a: tree }
    return tree
  }
  if (name === 'atNodes') return Array.from({ length: contract.limits.maxNodes - 1 }, () => 0)
  if (name === 'deep') {
    let tree = 'leaf'
    for (let level = 0; level <= contract.limits.maxDepth; level += 1) tree = { a: tree }
    return tree
  }
  if (name === 'wide') return Array.from({ length: contract.limits.maxNodes }, () => 0)
  throw new Error(`unknown build "${name}"`)
}

/** The right-hand side of a `const NAME = …` line in the router source. */
function declared(name) {
  const match = new RegExp(`\\bconst ${name} = ([^\\n]+)`).exec(routerSource)
  assert.ok(match, `router.ts no longer declares ${name}`)
  return match[1]
    .trim()
    .replace(/_/g, '')
    .split('*')
    .reduce((total, part) => total * Number(part.trim()), 1)
}

describe('flight decoder conformance', () => {
  it('holds the limits the shared table names', () => {
    assert.equal(declared('FLIGHT_MAX_NODES'), contract.limits.maxNodes)
    assert.equal(declared('FLIGHT_MAX_DEPTH'), contract.limits.maxDepth)
    assert.equal(declared('FLIGHT_BYTE_LIMIT'), contract.limits.byteLimit)
  })

  for (const testCase of contract.cases) {
    // `tree` comes out of `JSON.parse`, which defines `__proto__` as an own
    // property rather than setting the prototype. An object literal written
    // here would not: `{ __proto__: 1 }` in source assigns the prototype and
    // the unsafe-key cases would test nothing.
    const tree = testCase.build ? build(testCase.build) : testCase.tree
    const payload = envelope(tree)

    it(`${testCase.accept ? 'accepts' : 'refuses'} ${testCase.name}`, () => {
      if (testCase.accept) {
        assert.deepEqual(decodeFlight(payload, ARTIFACT_VERSION, ROUTE), tree)
        return
      }
      assert.throws(() => decodeFlight(payload, ARTIFACT_VERSION, ROUTE))
    })
  }

  it('refuses an envelope that names another protocol, version, artifact, or route', () => {
    for (const override of [
      { protocol: 'other' },
      { protocolVersion: contract.protocolVersion + 1 },
      { manifestVersion: 'fedcba9876543210' },
      { route: '/elsewhere' },
    ]) {
      assert.throws(
        () => decodeFlight(envelope({ ok: true }, override), ARTIFACT_VERSION, ROUTE),
        Object.keys(override)[0],
      )
    }
  })

  it('refuses a payload that is not an object', () => {
    assert.throws(() => decodeFlight('"a string"', ARTIFACT_VERSION, ROUTE))
    assert.throws(() => decodeFlight('[]', ARTIFACT_VERSION, ROUTE))
  })
})
