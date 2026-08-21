/**
 * The shared tables the serverless handler keeps its own copy of.
 *
 * `tests/fixtures/static-asset-conformance.json` and
 * `tests/fixtures/security-headers-conformance.json` were each written because
 * a table existed in two languages and drifted. Both were then replayed by the
 * Rust host and by `@ruvyxa/core` — and neither by
 * `packages/ruvyxa/runtime/serverless-handler.mjs`, which holds a third copy of
 * each and is the code that actually runs in every deployed serverless build.
 *
 * A copy outside the gate is the arrangement the fixtures exist to prevent, so
 * the handler replays them here.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const handlerPath = path.join(workspaceRoot, 'packages/ruvyxa/runtime/serverless-handler.mjs')

const { DEFAULT_SECURITY_HEADERS, isStaticAssetPath } = await import(
  `file://${handlerPath.replaceAll('\\', '/')}`
)

const handlerSource = readFileSync(handlerPath, 'utf8')

function fixture(name) {
  return JSON.parse(readFileSync(path.join(workspaceRoot, 'tests/fixtures', name), 'utf8'))
}

describe('serverless handler static asset table', () => {
  const { staticAssetExtensions } = fixture('static-asset-conformance.json')

  it('recognises every extension the shared table names', () => {
    for (const extension of staticAssetExtensions) {
      assert.equal(
        isStaticAssetPath(`/assets/file.${extension}`),
        true,
        `.${extension} is in the shared table but this host routes it as a page`,
      )
    }
  })

  it('holds no extension the shared table does not name', () => {
    // Behaviour alone cannot see an extra entry — there is no candidate to
    // probe with — so the declared set is read from the source, the same way
    // the client bootstrap contract reads its Rust mirror.
    const declared = handlerSource.match(
      /const STATIC_ASSET_EXTENSIONS = new Set\(\[([\s\S]*?)\]\)/,
    )
    assert.ok(declared, 'STATIC_ASSET_EXTENSIONS not found in serverless-handler.mjs')
    const entries = [...declared[1].matchAll(/'([^']+)'/g)].map((match) => match[1])

    assert.deepEqual(
      [...entries].sort(),
      [...staticAssetExtensions].sort(),
      'an extension this host serves and the shared table omits makes the same URL a page elsewhere',
    )
  })

  it('still routes an extension nobody declared as a page', () => {
    // The table decides which misses are refused outright rather than handed to
    // a dynamic route. Treating an undeclared extension as an asset would let
    // `/[slug]` stop answering a real page whose slug happens to contain a dot.
    for (const pathname of ['/blog/v1.2.3', '/docs/readme.txt', '/x/file.unknown']) {
      assert.equal(isStaticAssetPath(pathname), false, pathname)
    }
  })
})

describe('serverless handler security header table', () => {
  const { headers } = fixture('security-headers-conformance.json')

  it('sends the shared default for every header the fixture names', () => {
    // Names are compared case-insensitively because HTTP header names are;
    // values exactly, because a changed value stops being strippable by the
    // hosts that only remove a header still holding its default.
    const declared = new Map(
      Object.entries(DEFAULT_SECURITY_HEADERS).map(([name, value]) => [name.toLowerCase(), value]),
    )

    for (const [name, value] of Object.entries(headers)) {
      assert.equal(
        declared.get(name.toLowerCase()),
        value,
        `${name} differs here from the shared default, so the same site is protected differently depending on where it is deployed`,
      )
    }
  })

  it('adds no default header the shared table does not name', () => {
    const expected = Object.keys(headers).map((name) => name.toLowerCase())
    assert.deepEqual(
      Object.keys(DEFAULT_SECURITY_HEADERS)
        .map((name) => name.toLowerCase())
        .sort(),
      [...expected].sort(),
    )
  })
})
