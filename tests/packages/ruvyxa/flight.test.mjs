import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import {
  clientReference,
  decodeFlightPayload,
  encodeFlightPayload,
  FLIGHT_PROTOCOL,
  FLIGHT_PROTOCOL_VERSION,
} from '../../../packages/ruvyxa/runtime/flight.mjs'

describe('Flight transport contract', () => {
  it('round-trips deterministic supported values and client references', () => {
    const version = '0123456789abcdef'
    const encoded = encodeFlightPayload({
      manifestVersion: version,
      route: '/docs',
      tree: { z: 1, child: clientReference('m_0123456789abcdef', { label: 'Open' }) },
    })
    assert.match(encoded, new RegExp(`^\\{\"protocol\":\"${FLIGHT_PROTOCOL}`))
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
