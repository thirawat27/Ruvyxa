import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const handlerModule = path.join(workspaceRoot, 'packages/ruvyxa/runtime/serverless-handler.mjs')

const { createHandler } = await import(`file://${handlerModule.replaceAll('\\', '/')}`)

const fixture = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/cors-conformance.json'), 'utf8'),
)

const ORIGIN = 'https://app.example'
const route = { id: 'api', path: '/api', kind: 'api', file: 'api.ts', render: {} }

function handlerWithCors(cors) {
  return createHandler({
    routes: [route],
    middleware: { builtin: { cors } },
    importPage: async () => ({}),
    importApi: async () => ({ GET: () => new Response('ok') }),
  })
}

/** A preflight for a method a project would have to opt into. */
function preflight(handler, origin = ORIGIN) {
  return handler(
    new Request('https://worker.example/api', {
      method: 'OPTIONS',
      headers: { origin, 'access-control-request-method': 'PUT' },
    }),
  )
}

function actual(handler, origin = ORIGIN) {
  return handler(new Request('https://worker.example/api', { headers: { origin } }))
}

describe('built-in CORS conformance', () => {
  it('sends no method or header allowance the project did not name', async () => {
    // The Rust config used to fill `methods` with an implicit
    // GET/POST/PUT/DELETE/OPTIONS while this host had no such default, so the
    // same project answered this exact preflight one way under `ruvyxa dev`
    // and another way once deployed. Neither host defaults now, and this is
    // the assertion that keeps the one that did from coming back.
    assert.deepEqual(fixture.defaults.methods, [], 'the fixture must declare no default methods')
    assert.deepEqual(fixture.defaults.headers, [])

    const response = await preflight(handlerWithCors({ origins: [ORIGIN] }))

    assert.equal(response.headers.get('access-control-allow-origin'), ORIGIN)
    assert.equal(response.headers.get('access-control-allow-methods'), null)
    assert.equal(response.headers.get('access-control-allow-headers'), null)
  })

  it('caches a preflight for the fixture default when the project names no maxAge', async () => {
    const response = await preflight(handlerWithCors({ origins: [ORIGIN] }))
    assert.equal(
      response.headers.get('access-control-max-age'),
      String(fixture.defaults.maxAge),
      'a default that differs between hosts makes one of them re-preflight every request',
    )
  })

  it('grants no credentials the project did not ask for', async () => {
    assert.equal(fixture.defaults.credentials, false)
    const response = await actual(handlerWithCors({ origins: [ORIGIN] }))
    assert.equal(response.headers.get('access-control-allow-credentials'), null)
  })

  it('puts the negotiation headers on the preflight response only', async () => {
    const cors = {
      origins: [ORIGIN],
      methods: ['GET', 'PUT'],
      headers: ['Content-Type'],
      credentials: true,
      maxAge: 3600,
    }
    const handler = handlerWithCors(cors)
    const preflightResponse = await preflight(handler)
    const actualResponse = await actual(handler)

    for (const name of fixture.headerPlacement.preflightOnly) {
      assert.ok(preflightResponse.headers.has(name), `preflight is missing ${name}`)
      assert.equal(
        actualResponse.headers.get(name),
        null,
        `${name} answers a preflight question and does not belong on an actual response`,
      )
    }
    for (const name of fixture.headerPlacement.both) {
      assert.ok(preflightResponse.headers.has(name), `preflight is missing ${name}`)
      assert.ok(actualResponse.headers.has(name), `actual response is missing ${name}`)
    }
  })

  it('marks a rejected origin as origin-dependent for shared caches', async () => {
    assert.equal(fixture.varyOriginOnRejectedOrigin, true)
    const response = await actual(handlerWithCors({ origins: [ORIGIN] }), 'https://evil.example')

    assert.equal(response.headers.get('access-control-allow-origin'), null)
    assert.match(response.headers.get('vary') ?? '', /Origin/i)
  })

  it('grants no origin credentialed access through a wildcard', async () => {
    // The native host rejects this configuration before it binds a port. A
    // deployed function has no startup to fail in, so it fails the only way it
    // can: no origin is allowed and no CORS header is sent.
    assert.equal(fixture.credentialedWildcard.allowsAnyOrigin, false)
    const handler = handlerWithCors({ origins: ['*'], credentials: true })

    for (const origin of [ORIGIN, 'https://evil.example']) {
      const response = await actual(handler, origin)
      assert.equal(response.headers.get('access-control-allow-origin'), null, origin)
      assert.equal(response.headers.get('access-control-allow-credentials'), null, origin)
    }
  })

  it('is rejected at startup by the native host', () => {
    // The fixture claims the two hosts refuse a credentialed wildcard at the
    // earliest point each can. This half of that claim lives in Rust, so what
    // is checkable from here is that the rejection is still written down.
    assert.equal(fixture.credentialedWildcard.nativeRejectsAtStartup, true)
    const stack = readFileSync(
      path.join(workspaceRoot, 'crates/ruvyxa_middleware/src/stack.rs'),
      'utf8',
    )
    assert.match(stack, /CORS credentials cannot be enabled with the wildcard origin/)
  })
})
