import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const runtimeUrl = (file) =>
  `file://${path.join(workspaceRoot, 'packages/ruvyxa/runtime', file).replaceAll('\\', '/')}`

const { RESERVED_FRAMEWORK_PATHS, createPluginRegistry } = await import(
  runtimeUrl('plugin-http.mjs')
)
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

/** Endpoints `createHandler` claims before route matching. */
const dispatched = contract.endpoints.filter((endpoint) => endpoint.serverless === 'dispatch')

/** The host every probe below addresses, and the origin a browser would send. */
const PROBE_HOST = 'example.test'
const SAME_ORIGIN = `https://${PROBE_HOST}`

/** The three answers `requiredOrigin` and `rateLimited` may give. */
const GUARD_VALUES = new Set(['both', 'native', 'none'])

/**
 * The headers a dispatch probe sends.
 *
 * An endpoint that declares an origin guard is fail-closed when neither
 * `Origin` nor `Sec-Fetch-Site` is present, so a probe that sends neither is not
 * the request any real caller makes: the RSC client runtime and `@ruvyxa/react`'s
 * router both run in a browser, and a browser always sends one of the two.
 */
function probeHeaders(endpoint, probe) {
  const headers = { ...probe.requiredHeaders }
  if (endpoint.requiredOrigin !== 'none') {
    headers.host = PROBE_HOST
    headers.origin = SAME_ORIGIN
  }
  return headers
}

/**
 * A request that reaches each guarded endpoint's guard.
 *
 * The dispatch probes stop earlier than the guard on purpose — they ask only
 * whether the path is claimed at all — so a guard needs a request the endpoint
 * accepts as far as the check under test. One entry per dispatched endpoint that
 * declares a guard; an endpoint that declares one with no entry here fails
 * rather than being skipped.
 */
const guardedRequests = {
  '/__ruvyxa/action': {
    method: 'POST',
    query: '?path=/&name=save',
    headers: { 'content-type': 'application/json' },
    body: '{}',
  },
  '/__ruvyxa/rsc': {
    method: 'POST',
    query: '?path=/',
    headers: { 'x-ruvyxa-rsc': '1', 'x-ruvyxa-action': 'sample-module#action' },
    body: '',
  },
}

/** Send one guarded request, with `origin` the only thing a caller varies. */
function sendGuarded(handler, path, origin) {
  const spec = guardedRequests[path]
  return handler(
    new Request(`https://${PROBE_HOST}${path}${spec.query}`, {
      method: spec.method,
      headers: { host: PROBE_HOST, origin, ...spec.headers },
      body: spec.body,
    }),
  )
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

  // The socket transports a plugin may claim. This half runs first, inside the
  // plugin host; the Axum half re-checks the descriptor it is handed, because
  // a bad path there panics matchit inside `Router::route` rather than
  // producing a diagnostic. Both halves were a denylist of the axum 0.7
  // wildcard alphabet while the workspace ran axum 0.8, so `/{room}` passed
  // both. The Rust replay is
  // `a_transport_path_is_accepted_or_refused_as_the_contract_says`.
  it('agrees with the native host on which transport paths a plugin may claim', async () => {
    assert.ok(contract.transportPaths.length > 0, 'the transport path table must not be empty')

    for (const capability of ['realtime@1', 'presence@1']) {
      for (const { path: claimed, valid, why } of contract.transportPaths) {
        const registry = createPluginRegistry({
          plugins: [
            {
              name: 'transport',
              register({ native }) {
                native.claim(capability, { path: claimed })
              },
            },
          ],
        })

        if (valid) {
          const built = await registry
          assert.equal(
            built.capabilities.get(capability).path,
            claimed,
            `${capability} must accept ${claimed}: ${why}`,
          )
        } else {
          await assert.rejects(
            registry,
            TypeError,
            `${capability} must refuse ${JSON.stringify(claimed)}: ${why}`,
          )
        }
      }
    }
  })

  it('claims every dispatched endpoint before route matching', async () => {
    const handler = handlerWithNoRoutes()
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
          new Request(`https://${PROBE_HOST}${endpoint.path}${query}`, {
            method,
            headers: probeHeaders(endpoint, probe),
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

    for (const endpoint of dispatched) {
      for (const probe of endpoint.probes ?? [endpoint.probe ?? {}]) {
        const required = probe.requiredHeaders ?? {}
        for (const omitted of Object.keys(required)) {
          const headers = probeHeaders(endpoint, probe)
          delete headers[omitted]
          const query = probe.query ? `?${probe.query}` : ''
          const response = await handler(
            new Request(`https://${PROBE_HOST}${endpoint.path}${query}`, {
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

  it('declares, per dispatched endpoint, which host runs which guard', () => {
    // `requiredHeaders` can express a header gate and nothing else, which is how
    // `/__ruvyxa/action` came to run four guards and `/__ruvyxa/rsc` one with
    // this table green: the two hosts were held level on a rule that was not the
    // load-bearing one. Every dispatched endpoint answers both questions, so a
    // new one cannot arrive without deciding, and a misspelling is not a quiet
    // "no".
    for (const endpoint of dispatched) {
      for (const field of ['requiredOrigin', 'rateLimited']) {
        assert.ok(
          GUARD_VALUES.has(endpoint[field]),
          `${endpoint.path} must declare ${field} as one of both/native/none, not ${JSON.stringify(
            endpoint[field],
          )}`,
        )
      }
    }
  })

  it('runs every origin guard the contract says this host runs', async () => {
    const handler = handlerWithNoRoutes()

    for (const endpoint of dispatched.filter((e) => e.requiredOrigin !== 'none')) {
      assert.ok(
        guardedRequests[endpoint.path],
        `${endpoint.path} declares an origin guard but no request in this suite reaches it`,
      )
      const crossSite = await sendGuarded(handler, endpoint.path, 'https://evil.test')
      if (endpoint.requiredOrigin === 'both') {
        assert.equal(
          crossSite.status,
          403,
          `${endpoint.path} must refuse a cross-site call on this host too`,
        )
      } else {
        // The other direction, so `native` cannot go stale. `/__ruvyxa/rsc` is
        // guarded on the Axum host and not here: `handleRscPayload` and
        // `handleRscAction` still check the navigation header alone, which the
        // built-in CORS layer can make answerable. When that lands, this
        // assertion fails and the contract moves to `both`.
        assert.notEqual(
          crossSite.status,
          403,
          `${endpoint.path} refuses a cross-site call on this host; change requiredOrigin to "both"`,
        )
      }
      const sameOrigin = await sendGuarded(handler, endpoint.path, SAME_ORIGIN)
      assert.notEqual(
        sameOrigin.status,
        403,
        `a same-origin call to ${endpoint.path} must still be served`,
      )
    }
  })

  it('runs every rate limiter the contract says this host runs', async () => {
    // Two hits, so the ceiling is reached in three requests rather than 601.
    const handler = createHandler({
      routes: [],
      importPage: async () => ({ render: async () => '<html></html>' }),
      importApi: async () => ({}),
      importAction: async () => null,
      security: { actionRateLimit: { max: 2, window: 60 } },
    })

    for (const endpoint of dispatched.filter((e) => e.rateLimited !== 'none')) {
      assert.ok(
        guardedRequests[endpoint.path],
        `${endpoint.path} declares a rate limiter but no request in this suite reaches it`,
      )
      const statuses = []
      for (let attempt = 0; attempt < 3; attempt++) {
        statuses.push((await sendGuarded(handler, endpoint.path, SAME_ORIGIN)).status)
      }
      if (endpoint.rateLimited === 'both') {
        assert.equal(
          statuses.at(-1),
          429,
          `${endpoint.path} must refuse a call past its ceiling; saw ${statuses.join(', ')}`,
        )
      } else {
        assert.ok(
          !statuses.includes(429),
          `${endpoint.path} is rate limited on this host; change rateLimited to "both"`,
        )
      }
    }
  })

  it('decides the framework endpoints ahead of the plugin stage', async () => {
    // RTMS-05. On the native host the framework endpoints are axum routes and
    // the plugin-bearing handler is the fallback, so a reserved path never
    // reaches `apply_request_plugins`. This host wrapped the plugin stage around
    // everything, so an `http.onRequest({ match: ['*'] })` hook guarded
    // `POST /__ruvyxa/action` when deployed and did not guard it under
    // `dev`/`start` -- the dangerous direction for a security plugin, whose
    // author develops against the host that does not exercise the guard.
    //
    // `handlerWithNoRoutes` builds its handler with no `pluginHttp` at all,
    // which is why the contract could not see this.
    const seen = []
    const handler = createHandler({
      routes: [],
      importPage: async () => ({ render: async () => '<html></html>' }),
      importApi: async () => ({}),
      importAction: async () => null,
      optimizeImage: async () => new Response('image', { status: 200 }),
      pluginHttp: async (request) => {
        seen.push(new URL(request.url).pathname)
        return new Response('plugin', { status: 418 })
      },
    })

    for (const endpoint of dispatched) {
      for (const probe of endpoint.probes ?? [endpoint.probe ?? {}]) {
        const query = probe.query ? `?${probe.query}` : ''
        const response = await handler(
          new Request(`https://${PROBE_HOST}${endpoint.path}${query}`, {
            method: probe.method ?? 'GET',
            headers: probeHeaders(endpoint, probe),
          }),
        )
        assert.notEqual(
          response.status,
          418,
          `${endpoint.path} was answered by a plugin; the framework owns this path`,
        )
      }
    }
    assert.deepEqual(seen, [], 'no reserved endpoint may reach the plugin stage')

    // The other direction, so skipping the stage cannot become skipping it for
    // everything: an ordinary application path still runs plugin hooks.
    const ordinary = await handler(new Request(`https://${PROBE_HOST}/about`))
    assert.equal(ordinary.status, 418)
    assert.deepEqual(seen, ['/about'])
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
