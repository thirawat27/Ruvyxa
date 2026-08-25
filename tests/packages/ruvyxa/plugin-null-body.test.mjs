import assert from 'node:assert/strict'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const runtimeUrl = (file) =>
  `file://${path.join(workspaceRoot, 'packages/ruvyxa/runtime', file).replaceAll('\\', '/')}`

const { dispatchPluginResponse, isNullBodyStatus } = await import(runtimeUrl('plugin-http.mjs'))

/**
 * The response hook every page of the documentation shows.
 *
 * It rebuilds the response to add a header, which is the only way to do it
 * without a helper — `Response.headers` is immutable. `response.body` is `null`
 * for a status that carries no body, and `new Response` accepts that; it throws
 * for anything else, including an empty string.
 */
const addHeaderByRebuilding = {
  plugin: 'test-observability',
  match: ['*'],
  handler({ response }) {
    const headers = new Headers(response.headers)
    headers.set('x-test-plugin', 'active')
    return new Response(response.body, {
      status: response.status,
      statusText: response.statusText,
      headers,
    })
  },
}

describe('null-body statuses through a plugin response hook', () => {
  it('names exactly the statuses the fetch specification forbids a body on', () => {
    for (const status of [101, 103, 204, 205, 304]) {
      assert.equal(isNullBodyStatus(status), true, `${status} carries no body`)
    }
    for (const status of [200, 201, 202, 206, 301, 400, 404, 500]) {
      assert.equal(isNullBodyStatus(status), false, `${status} may carry a body`)
    }
  })

  it('lets the documented rebuild-the-response hook survive every one of them', async () => {
    // 101 and 103 are excluded because `new Response` refuses to construct them
    // at all — a host produces them, a plugin never receives one.
    for (const status of [204, 205, 304]) {
      const response = await dispatchPluginResponse(
        { root: workspaceRoot, httpResponse: [addHeaderByRebuilding] },
        new Request('http://ruvyxa.local/api/verbs', { method: 'OPTIONS' }),
        new Response(null, { status }),
      )
      assert.equal(response.status, status)
      assert.equal(response.headers.get('x-test-plugin'), 'active')
    }
  })

  it('keeps the rule in one place, so the two hosts cannot disagree about it', async () => {
    // `plugin-runtime.mjs` decodes a Response out of the NDJSON payload the
    // Rust host sends and is where the empty-string body came from. It has to
    // import the predicate rather than carry its own copy: a second list is how
    // a status added to one host stays missing from the other.
    const { readFile } = await import('node:fs/promises')
    const source = await readFile(
      path.join(workspaceRoot, 'packages/ruvyxa/runtime/plugin-runtime.mjs'),
      'utf8',
    )
    assert.match(source, /isNullBodyStatus/)
    assert.doesNotMatch(source, /function isNullBodyStatus/)
  })
})
