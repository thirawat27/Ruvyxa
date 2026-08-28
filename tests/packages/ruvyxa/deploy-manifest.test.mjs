/**
 * The JavaScript half of `tests/fixtures/deploy-output-conformance.json`.
 *
 * Three implementations decide how a finished build is served: the Rust writer
 * (`crates/ruvyxa_cli/src/deploy_manifest.rs`), the reader every adapter uses
 * (`packages/@ruvyxa/core/src/deploy-manifest.ts`), and the request handler
 * every deployed build runs (`packages/ruvyxa/runtime/serverless-handler.mjs`).
 * They cannot share code — one is Rust, one is a published TypeScript package,
 * and one is a runtime `.mjs` file that resolves no workspace specifiers — so
 * they share this table instead.
 *
 * What goes wrong when they drift is silent, which is why the table exists at
 * all: an adapter that publishes an ISR page as a static file produces a site
 * that serves its build-time snapshot forever, with no error anywhere.
 *
 * The Rust half is `serve_mode_matches_the_shared_conformance_table` and its
 * two siblings in `crates/ruvyxa_cli/src/deploy_manifest.rs`.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const fixture = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/deploy-output-conformance.json'), 'utf8'),
)

const core = await import(
  `file://${path.join(workspaceRoot, 'packages/@ruvyxa/core/dist/index.js').replaceAll('\\', '/')}`
)
const handler = await import(
  `file://${path
    .join(workspaceRoot, 'packages/ruvyxa/runtime/serverless-handler.mjs')
    .replaceAll('\\', '/')}`
)

describe('deployment output conformance', () => {
  it('classifies every route the way the shared table does', () => {
    const cases = fixture.serve.cases
    assert.ok(cases.length > 0, 'the fixture must carry cases')
    for (const testCase of cases) {
      assert.equal(
        core.routeServeMode(testCase.kind, testCase.strategy, testCase.prerendered),
        testCase.expect,
        `${testCase.kind} ${testCase.strategy} (prerendered: ${testCase.prerendered}) — ${testCase.why}`,
      )
    }
  })

  it('sends the document cache-control the shared table names, from both implementations', () => {
    for (const testCase of fixture.documentCacheControl.cases) {
      assert.equal(
        core.documentCacheControl(testCase.strategy, testCase.revalidate),
        testCase.expect,
        `@ruvyxa/core: ${testCase.strategy}`,
      )
      assert.equal(
        handler.documentCacheControl(testCase.strategy, testCase.revalidate),
        testCase.expect,
        `serverless-handler.mjs: ${testCase.strategy}`,
      )
    }
  })

  it('validates exactly the documents the shared table names', () => {
    // Membership only. The validator's value is host-local — the native server
    // hashes with blake3, this one with SHA-256 — because a validator is opaque
    // and scoped to the origin that issued it. Which documents get one is the
    // part that has to agree: validating an `ssr` document would answer `304`
    // for a page rendered for somebody else.
    const validated = new Set(handler.DOCUMENT_VALIDATOR_STRATEGIES)
    for (const testCase of fixture.documentValidator.cases) {
      assert.equal(
        validated.has(testCase.strategy),
        testCase.expect,
        `serverless-handler.mjs: ${testCase.strategy}`,
      )
    }
    assert.equal(
      validated.size,
      fixture.documentValidator.cases.filter((testCase) => testCase.expect).length,
      'the handler must validate nothing the table does not name',
    )
  })

  it('carries the same cache-control per asset class as the servers send', () => {
    assert.equal(fixture.assetClasses.client.cacheControl, core.IMMUTABLE_CACHE_CONTROL)
    assert.equal(fixture.assetClasses.asset.cacheControl, core.PUBLIC_ASSET_CACHE_CONTROL)
    assert.equal(fixture.assetClasses.document.cacheControl, core.DOCUMENT_CACHE_CONTROL)
    assert.equal(fixture.assetClasses.document.cacheControl, handler.DOCUMENT_CACHE_CONTROL)
  })

  it('derives the strategies that must not be published as files', () => {
    // Not compared against a literal list: the point of deriving them is that
    // no list has to be maintained. What must hold is the property — a
    // strategy is unpublishable exactly when a pre-rendered page of that
    // strategy still has to reach the function.
    const unpublishable = core.nonPublishableStrategies()
    for (const strategy of ['ssr', 'ssg', 'isr', 'csr', 'ppr']) {
      assert.equal(
        unpublishable.includes(strategy),
        core.routeServeMode('page', strategy, true) === 'function',
        strategy,
      )
    }
    assert.ok(unpublishable.includes('isr'), 'ISR must never be published as a static file')
    assert.ok(unpublishable.includes('ppr'), 'PPR must never be published as a static file')
  })

  it('names only endpoints the framework has actually reserved', () => {
    // A host that serves the publish directory before invoking the function
    // has to forward these, so an entry naming a path nothing reserves would
    // send it a request no route answers.
    const reserved = new Set(
      JSON.parse(
        readFileSync(
          path.join(workspaceRoot, 'tests/fixtures/framework-endpoint-conformance.json'),
          'utf8',
        ),
      ).endpoints.map((endpoint) => endpoint.path),
    )
    const endpoints = Object.entries(fixture.reservedEndpoints).filter(
      ([name]) => name !== '$comment',
    )
    assert.ok(endpoints.length > 0)
    for (const [name, endpoint] of endpoints) {
      assert.ok(reserved.has(endpoint.replace(/\/$/, '')), `${name}: ${endpoint}`)
    }
  })

  it('refuses a manifest from a contract version it does not understand', () => {
    const manifest = { version: 1, framework: 'ruvyxa', routes: [] }
    assert.ok(core.parseDeployManifest(manifest))
    assert.equal(
      core.parseDeployManifest({ ...manifest, version: core.DEPLOY_MANIFEST_VERSION + 1 }),
      null,
      'a newer contract must be refused rather than partially read',
    )
    assert.equal(core.parseDeployManifest({ ...manifest, framework: 'other' }), null)
    assert.equal(core.parseDeployManifest({ ...manifest, routes: undefined }), null)
    assert.equal(core.parseDeployManifest(null), null)
  })

  it('falls back to derived header rules when a build wrote no manifest', () => {
    const derived = core.deployHeaderRules(null)
    assert.equal(derived[0].headers['cache-control'], core.IMMUTABLE_CACHE_CONTROL)
    assert.equal(derived[0].source, `${core.CLIENT_BUNDLE_PREFIX}(.*)`)
    assert.equal(derived.at(-1).headers['cache-control'], core.PUBLIC_ASSET_CACHE_CONTROL)

    const fromManifest = core.deployHeaderRules({
      headers: [{ source: '/x/(.*)', headers: { 'cache-control': 'public, max-age=1' } }],
    })
    assert.equal(fromManifest[0].source, '/x/(.*)')
  })
})
