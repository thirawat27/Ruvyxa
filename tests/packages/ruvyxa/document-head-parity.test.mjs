/**
 * The deployed document writer composes the head the native hosts compose.
 *
 * A page inside a deployed function bundle is rendered by the source
 * `documentAssetsPrelude` generates, not by anything in `crates/`. That writer
 * had the viewport default and not the icon one, so a deployed build served its
 * pre-rendered pages with an icon link and every request-time render without
 * it: the browser fell back to `/favicon.ico`, which the `ruvyxa.png`
 * convention every scaffolded project ships does not answer.
 *
 * The rule is replayed against
 * `crates/ruvyxa_dev_server/src/static_assets.rs` through
 * `tests/fixtures/document-head-conformance.json`. The generated source is
 * imported and executed rather than pattern-matched, because generating text
 * that does not parse is the failure mode this file exists inside: a stray
 * backtick in one of those doc comments produces a bundle that throws on load
 * and no other check compiles it.
 */

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { documentAssetsPrelude } from '../../../packages/ruvyxa/runtime/entry-templates.mjs'
import { escapeHtmlAttribute } from '../../../packages/ruvyxa/runtime/serverless-handler.mjs'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const fixture = JSON.parse(
  readFileSync(path.join(repoRoot, 'tests/fixtures/document-head-conformance.json'), 'utf8'),
)

/**
 * Load the generated writer for one build's resolved head fragments.
 *
 * Each case gets its own module because `assetLinks` is baked into the source
 * as a constant — that is the whole point of it, a deployed function having no
 * `public/` to look in — so it cannot be varied after generation.
 */
async function documentHeadWriter(assetLinks, pluginHead = '') {
  const source = documentAssetsPrelude('', { assetLinks, pluginHead })
  const module = await import(
    `data:text/javascript,${encodeURIComponent(`${source}\nexport { __ruvyxaDocumentHead }`)}`
  )
  return module.__ruvyxaDocumentHead
}

describe('document head defaults', () => {
  it('the fixture and the generated constant declare the same viewport', async () => {
    const write = await documentHeadWriter('')
    assert.equal(write('<html><head></head></html>', ''), fixture.viewportMeta)
  })

  for (const testCase of fixture.cases) {
    it(testCase.name, async () => {
      const write = await documentHeadWriter(testCase.assetLinks)
      assert.equal(write(testCase.document, ''), testCase.expect)
    })
  }

  /**
   * The two additions this writer makes that the native one has no equivalent
   * for, because they are already in a pre-rendered page by the time it is
   * written to disk. They are appended in the order the live pipeline composes
   * them: defaults, then the plugins' entries, then whatever the render itself
   * contributed.
   */
  it('a plugin head declaration is carried into a deployed render', async () => {
    const write = await documentHeadWriter(
      '<link rel="icon" type="image/png" href="/ruvyxa.png">',
      '<script src="https://example.test/a.js" defer></script>',
    )
    const head = write('<html><head></head></html>', '<link rel="stylesheet" href="/s.css">')

    assert.match(head, /example\.test\/a\.js/)
    assert.ok(head.indexOf('ruvyxa.png') < head.indexOf('example.test'))
    assert.ok(head.indexOf('example.test') < head.indexOf('/s.css'))
  })
})

/**
 * The escaping half of the same fixture, against both JavaScript copies.
 *
 * `escape_html` on the native host says it is the one place this rule lives.
 * It is not: the prelude `entry-templates.mjs` generates carries
 * `__ruvyxaEscapeAttribute`, and `serverless-handler.mjs` carries
 * `escapeHtmlAttribute`, and a deployed page goes through one of those rather
 * than through anything in `crates/`. All three were missing `'`, which was
 * inert only for as long as every call site happened to be element text or a
 * double-quoted attribute. `crates/ruvyxa_dev_server/src/static_assets.rs`
 * replays this same table on the other side.
 */
describe('the HTML escaping rule, across every copy of it', () => {
  const cases = fixture.escaping.cases

  it('has a table to replay', () => {
    assert.ok(Array.isArray(cases) && cases.length > 0, 'an empty table asserts nothing')
  })

  it('is the same rule in the generated entry prelude', async () => {
    const source = documentAssetsPrelude('', { assetLinks: '', pluginHead: '' })
    const module = await import(
      `data:text/javascript,${encodeURIComponent(`${source}\nexport { __ruvyxaEscapeAttribute }`)}`
    )
    for (const { name, input, expect } of cases) {
      assert.equal(module.__ruvyxaEscapeAttribute(input), expect, name)
    }
  })

  it('is the same rule in the deployed request-time writer', () => {
    for (const { name, input, expect } of cases) {
      assert.equal(escapeHtmlAttribute(input), expect, name)
    }
  })
})
