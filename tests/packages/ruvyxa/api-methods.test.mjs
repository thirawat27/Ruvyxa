import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const runtimeUrl = (file) =>
  `file://${path.join(workspaceRoot, 'packages/ruvyxa/runtime', file).replaceAll('\\', '/')}`

const { methodNotAllowed, routeMethods, selectRouteHandler } = await import(
  runtimeUrl('api-methods.mjs')
)

const GET_ONLY = { GET: () => Response.json({ ok: true }) }
const READ_WRITE = { GET: () => new Response('r'), POST: () => new Response('w') }
const OWN_HEAD = { GET: () => new Response('r'), HEAD: () => new Response(null, { status: 204 }) }

describe('which export answers a route request', () => {
  it('answers HEAD with the route’s GET when it declares none of its own', () => {
    const selected = selectRouteHandler(GET_ONLY, 'HEAD')
    assert.equal(selected.handler, GET_ONLY.GET)
    assert.equal(selected.method, 'GET')
    // The headers GET would send, without the content — RFC 9110 §9.3.2. Every
    // uptime monitor, link checker, and CDN revalidation sends HEAD first, and
    // all three hosts used to refuse it with a 405.
    assert.equal(selected.omitBody, true)
  })

  it('prefers a declared HEAD over the fallback', () => {
    const selected = selectRouteHandler(OWN_HEAD, 'HEAD')
    assert.equal(selected.handler, OWN_HEAD.HEAD)
    assert.equal(selected.method, 'HEAD')
    assert.equal(selected.omitBody, true)
  })

  it('keeps the body for every other method', () => {
    assert.equal(selectRouteHandler(READ_WRITE, 'post').omitBody, false)
    assert.equal(selectRouteHandler(READ_WRITE, 'GET').omitBody, false)
  })

  it('answers nothing for a method the route does not serve', () => {
    assert.equal(selectRouteHandler(GET_ONLY, 'DELETE'), null)
    assert.equal(selectRouteHandler({}, 'GET'), null)
  })
})

describe('what a refusal says', () => {
  it('lists the methods the route serves, in a fixed order', () => {
    // Fixed rather than sorted: `Allow` is compared byte-for-byte by caches,
    // and this repository bans `localeCompare` outright because it answers by
    // the host's ICU locale.
    assert.equal(methodNotAllowed(READ_WRITE, 'DELETE').allow, 'GET, HEAD, POST')
    assert.equal(methodNotAllowed(GET_ONLY, 'PUT').allow, 'GET, HEAD')
    assert.deepEqual(routeMethods(OWN_HEAD), ['GET', 'HEAD'])
  })

  it('names the method that was refused', () => {
    const refusal = methodNotAllowed(GET_ONLY, 'delete')
    assert.equal(refusal.status, 405)
    assert.equal(refusal.body, 'Method DELETE is not allowed')
  })

  it('says so even when the route exports nothing at all', () => {
    assert.equal(methodNotAllowed({}, 'GET').allow, '')
  })
})

describe('the three hosts that dispatch API routes', () => {
  // Each had its own `mod[method]` and its own bare 405. They agreed, and they
  // were wrong in the same two ways — which is what a rule copied three times
  // does. A fourth copy would be invisible until it drifted.
  const hosts = ['serverless-handler.mjs', 'worker-pool.mjs', 'api-renderer.mjs']

  it('all read the rule from one module', async () => {
    for (const host of hosts) {
      const source = await readFile(
        path.join(workspaceRoot, 'packages/ruvyxa/runtime', host),
        'utf8',
      )
      assert.match(source, /from '\.\/api-methods\.mjs'/, host)
      assert.doesNotMatch(source, /mod\[method(?:\.toUpperCase\(\))?\]/, host)
    }
  })

  it('is registered everywhere a runtime module has to be', async () => {
    // A new `runtime/*.mjs` that is missing from any of these is absent exactly
    // where nobody looks: the published tarball, a deployed function directory,
    // or the hash that decides whether prerendered output is stale.
    const manifest = JSON.parse(
      await readFile(path.join(workspaceRoot, 'packages/ruvyxa/package.json'), 'utf8'),
    )
    assert.ok(manifest.files.includes('runtime/api-methods.mjs'))

    const { HANDLER_RUNTIME_FILES } = await import(runtimeUrl('serverless-handler.mjs'))
    assert.ok(HANDLER_RUNTIME_FILES.includes('api-methods.mjs'))

    const artifactCache = await readFile(
      path.join(workspaceRoot, 'crates/ruvyxa_cli/src/artifact_cache.rs'),
      'utf8',
    )
    assert.match(artifactCache, /"api-methods\.mjs"/)
  })
})
