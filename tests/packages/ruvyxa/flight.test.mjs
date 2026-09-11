import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import {
  clientReference,
  decodeFlightPayload,
  encodeFlightPayload,
  FLIGHT_PROTOCOL,
  FLIGHT_PROTOCOL_VERSION,
} from '../../../packages/ruvyxa/runtime/flight.mjs'

const repoRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const contract = JSON.parse(
  readFileSync(path.join(repoRoot, 'tests/fixtures/flight-conformance.json'), 'utf8'),
)
const flightSource = readFileSync(path.join(repoRoot, 'packages/ruvyxa/runtime/flight.mjs'), 'utf8')

describe('Flight transport contract', () => {
  it('round-trips deterministic supported values and client references', () => {
    const version = '0123456789abcdef'
    const encoded = encodeFlightPayload({
      manifestVersion: version,
      route: '/docs',
      tree: { z: 1, child: clientReference('m_0123456789abcdef', { label: 'Open' }) },
    })
    assert.match(encoded, new RegExp(`^\\{"protocol":"${FLIGHT_PROTOCOL}`))
    const decoded = decodeFlightPayload(encoded, version)
    assert.equal(JSON.parse(encoded).protocolVersion, FLIGHT_PROTOCOL_VERSION)
    assert.equal(decoded.route, '/docs')
    assert.deepEqual({ ...decoded.tree.child.props }, { label: 'Open' })
  })

  it('rejects stale, malformed, cyclic, executable, and oversized values', () => {
    const version = '0123456789abcdef'
    const encoded = encodeFlightPayload({ manifestVersion: version, route: '/', tree: null })
    assert.throws(() => decodeFlightPayload(encoded, 'fedcba9876543210'), /version mismatch/)
    assert.throws(
      () => encodeFlightPayload({ manifestVersion: version, route: '/', tree: () => {} }),
      /unsupported function/,
    )
    const cyclic = {}
    cyclic.self = cyclic
    assert.throws(
      () => encodeFlightPayload({ manifestVersion: version, route: '/', tree: cyclic }),
      /cyclic/,
    )
    assert.throws(() => decodeFlightPayload(encoded, version, 1), /byte limit/)
  })

  it('rejects prototype-bearing and pollution-shaped objects', () => {
    const version = '0123456789abcdef'
    assert.throws(
      () => encodeFlightPayload({ manifestVersion: version, route: '/', tree: new Date() }),
      /plain objects/,
    )
    const unsafe = Object.create(null)
    Object.defineProperty(unsafe, '__proto__', { value: 'unsafe', enumerable: true })
    assert.throws(
      () => encodeFlightPayload({ manifestVersion: version, route: '/', tree: unsafe }),
      /unsafe object key/,
    )
  })
})

/**
 * The same table `packages/@ruvyxa/react/test/flight-decoder.test.mjs` replays.
 *
 * This module writes every payload and decodes one beside the encoder; the
 * browser's `decodeFlight` decodes the ones that arrive. Nothing outside this
 * file calls `decodeFlightPayload`, so the round trip above proved the format
 * agrees with itself — which is exactly what it cannot prove about the reader
 * that runs.
 */
describe('Flight wire conformance', () => {
  const MANIFEST_VERSION = '0123456789abcdef'
  const ROUTE = '/guide'

  /** The envelope both decoders are handed, byte for byte. */
  function envelope(tree) {
    return JSON.stringify({
      protocol: contract.protocol,
      protocolVersion: contract.protocolVersion,
      manifestVersion: MANIFEST_VERSION,
      route: ROUTE,
      tree,
    })
  }

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

  /** The right-hand side of a `const NAME = …` line in the module source. */
  function declared(name) {
    const match = new RegExp(`\\bconst ${name} = ([^\\n]+)`).exec(flightSource)
    assert.ok(match, `flight.mjs no longer declares ${name}`)
    return match[1]
      .trim()
      .replace(/_/g, '')
      .split('*')
      .reduce((total, part) => total * Number(part.trim()), 1)
  }

  it('holds the protocol and the limits the shared table names', () => {
    assert.equal(FLIGHT_PROTOCOL, contract.protocol)
    assert.equal(FLIGHT_PROTOCOL_VERSION, contract.protocolVersion)
    assert.equal(declared('MAX_NODES'), contract.limits.maxNodes)
    assert.equal(declared('MAX_DEPTH'), contract.limits.maxDepth)
    assert.equal(declared('DEFAULT_FLIGHT_LIMIT'), contract.limits.byteLimit)
  })

  for (const testCase of contract.cases) {
    // From `JSON.parse`, which defines `__proto__` as an own property. The
    // same literal written in source would set the prototype instead and the
    // unsafe-key cases would assert nothing.
    const tree = testCase.build ? build(testCase.build) : testCase.tree
    const payload = envelope(tree)

    it(`${testCase.accept ? 'accepts' : 'refuses'} ${testCase.name}`, () => {
      if (testCase.accept) {
        // Through JSON on both sides, because the two decoders deliberately
        // hand back different objects for the same bytes: this one rebuilds
        // every object with `Object.create(null)` and sorted keys so an
        // encode is deterministic, while the browser returns `JSON.parse`'s
        // own result. What has to agree is the data and the accept/refuse
        // answer, not the prototype.
        const decoded = decodeFlightPayload(payload, MANIFEST_VERSION).tree
        assert.deepEqual(JSON.parse(JSON.stringify(decoded)), JSON.parse(JSON.stringify(tree)))
        return
      }
      assert.throws(() => decodeFlightPayload(payload, MANIFEST_VERSION))
    })
  }

  it('rebuilds objects without a prototype, which the browser decoder does not', () => {
    // Recorded rather than left to be discovered: the divergence above is a
    // decision. This decoder is the one an encode goes through, so it
    // normalizes; `decodeFlight` in @ruvyxa/react feeds React directly and
    // returns what `JSON.parse` produced.
    const decoded = decodeFlightPayload(envelope({ b: 1, a: 2 }), MANIFEST_VERSION).tree
    assert.equal(Object.getPrototypeOf(decoded), null)
    assert.deepEqual(Object.keys(decoded), ['a', 'b'])
  })
})
