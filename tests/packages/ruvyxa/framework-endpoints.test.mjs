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
      const query = endpoint.probe?.query ? `?${endpoint.probe.query}` : ''
      const response = await handler(
        new Request(`https://example.test${endpoint.path}${query}`, {
          method: endpoint.probe?.method ?? 'GET',
        }),
      )
      assert.equal(
        await isRouteMiss(response),
        false,
        `${endpoint.path} fell through to route matching; the handler does not serve it`,
      )
    }
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
