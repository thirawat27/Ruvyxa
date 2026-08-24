import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const runtimeUrl = (file) =>
  `file://${path.join(workspaceRoot, 'packages/ruvyxa/runtime', file).replaceAll('\\', '/')}`

const { RESERVED_FRAMEWORK_PATHS } = await import(runtimeUrl('plugin-http.mjs'))
const { createHandler } = await import(runtimeUrl('serverless-handler.mjs'))
const contract = JSON.parse(
  await readFile(
    path.join(workspaceRoot, 'tests/fixtures/framework-endpoint-conformance.json'),
    'utf8',
  ),
)

/**
 * Build a handler with no application routes at all.
 *
 * Every path therefore reaches the generic route-miss unless the handler
 * claimed it first, which is exactly the property under test: `/__ruvyxa/action`
 * used to fall through to route matching and answer 404 like any unknown URL.
 */
function handlerWithNoRoutes() {
  return createHandler({
    routes: [],
    importPage: async () => ({ render: async () => '<html></html>' }),
    importApi: async () => ({}),
    importAction: async () => null,
    optimizeImage: async () => new Response('image', { status: 200 }),
  })
}

/** The response an unclaimed path produces: a bare 404 with no content type. */
async function isRouteMiss(response) {
  return response.status === 404 && (await response.clone().text()) === 'Not Found'
}

describe('framework endpoint conformance', () => {
  it('agrees with the native host on which paths are reserved', () => {
    const reserved = contract.endpoints
      .filter((endpoint) => endpoint.reserved)
      .map((endpoint) => endpoint.path)
    // Order is asserted too: RESERVED_FRAMEWORK_ROUTES in the Rust host is a
    // fixed-size array whose contents mirror the axum Router chain, and keeping
    // the three lists in one order makes a diff between them readable.
    assert.deepEqual([...RESERVED_FRAMEWORK_PATHS], reserved)
  })

  it('claims every dispatched endpoint before route matching', async () => {
    const handler = handlerWithNoRoutes()
    const dispatched = contract.endpoints.filter((endpoint) => endpoint.serverless === 'dispatch')
    assert.ok(dispatched.length > 0, 'the contract must dispatch at least one endpoint')

    for (const endpoint of dispatched) {
      // One probe per verb the endpoint answers. A single probe checks one verb
      // and says nothing about the others, which is exactly how `POST
      // /__ruvyxa/rsc` — every server-function call on a deployed
      // server-components page — returned 405 with this test green.
      for (const probe of endpoint.probes ?? [endpoint.probe ?? {}]) {
        const query = probe.query ? `?${probe.query}` : ''
        const method = probe.method ?? 'GET'
        const response = await handler(
          new Request(`https://example.test${endpoint.path}${query}`, {
            method,
            headers: probe.requiredHeaders ?? {},
          }),
        )
        assert.equal(
          await isRouteMiss(response),
          false,
          `${endpoint.path} fell through to route matching; the handler does not serve it`,
        )
        assert.notEqual(
          response.status,
          405,
          `${method} ${endpoint.path} is refused by the handler but listed in the contract`,
        )
      }
    }
  })

  it('refuses a dispatched endpoint that is missing a header the contract requires', async () => {
    // The other direction of the probe above. `/__ruvyxa/rsc` renders with the
    // visitor's cookies and runs server functions, and its only cross-origin
    // defence is a header a third-party page cannot set without a preflight
    // nothing answers -- there is no origin policy on this path the way there
    // is on `/__ruvyxa/action`. That gate was written twice, once per request
    // host, and held by neither this table nor anything else: a probe that
    // omitted the header got a 400 and passed the dispatch assertion, so a host
    // that stopped checking would have stayed green while answering a
    // cross-origin server-function call.
    const handler = handlerWithNoRoutes()
    let checked = 0

    for (const endpoint of contract.endpoints.filter((e) => e.serverless === 'dispatch')) {
      for (const probe of endpoint.probes ?? [endpoint.probe ?? {}]) {
        const required = probe.requiredHeaders ?? {}
        for (const omitted of Object.keys(required)) {
          const headers = { ...required }
          delete headers[omitted]
          const query = probe.query ? `?${probe.query}` : ''
          const response = await handler(
            new Request(`https://example.test${endpoint.path}${query}`, {
              method: probe.method ?? 'GET',
              headers,
            }),
          )
          assert.equal(
            response.status,
            400,
            `${probe.method ?? 'GET'} ${endpoint.path} without ${omitted} must be refused`,
          )
          checked++
        }
      }
    }

    assert.ok(checked > 0, 'the contract must require a header on at least one dispatched endpoint')
  })

  it('does not claim paths the contract does not list', async () => {
    const handler = handlerWithNoRoutes()
    const response = await handler(new Request('https://example.test/__ruvyxa/not-an-endpoint'))
    // Guards the assertion above: without this, a handler that answered every
    // `/__ruvyxa/*` path would pass the dispatch test while serving nothing.
    assert.equal(await isRouteMiss(response), true)
  })

  it('answers the action endpoint rather than routing it', async () => {
    const handler = handlerWithNoRoutes()
    const response = await handler(
      new Request('https://example.test/__ruvyxa/action?path=/x&name=y', { method: 'GET' }),
    )
    assert.equal(response.status, 405)
    assert.equal(response.headers.get('allow'), 'POST')
  })

  it('reports missing action support as 501 rather than 404', async () => {
    // A build artifact without an action registry has to be distinguishable
    // from a project that simply declares no action at that path.
    const handler = createHandler({
      routes: [],
      importPage: async () => ({ render: async () => '<html></html>' }),
      importApi: async () => ({}),
    })
    const response = await handler(
      new Request('https://example.test/__ruvyxa/action?path=/x&name=y', {
        method: 'POST',
        headers: { 'content-type': 'application/json', host: 'example.test' },
        body: '{}',
      }),
    )
    assert.equal(response.status, 501)
    assert.match(await response.text(), /RUV2211/)
  })
})
