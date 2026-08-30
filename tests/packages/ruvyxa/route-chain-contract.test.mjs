/**
 * The dev server's half of `tests/fixtures/route-chain-conformance.json`.
 *
 * A route's layout and template chains are discovered twice: `layout_chain()`
 * and `template_chain()` in `crates/ruvyxa_graph/src/lib.rs` answer for
 * `ruvyxa build`, and `collectLayouts()`/`collectTemplates()` in
 * `packages/ruvyxa/runtime/compiler.mjs` answer for `ruvyxa dev` and for every
 * route a deployed function composes. A layout one host wraps a page in and the
 * other does not is a page that has its document shell in production and loses
 * it locally.
 *
 * The Rust half is `route_chains_match_the_shared_conformance_table`.
 */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const modulePath = path.join(workspaceRoot, 'packages/ruvyxa/runtime/compiler.mjs')

const { collectLayouts, collectTemplates } = await import(
  `file://${modulePath.replaceAll('\\', '/')}`
)

const contract = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/route-chain-conformance.json'), 'utf8'),
)

function materialise(tree) {
  const root = mkdtempSync(path.join(os.tmpdir(), 'ruvyxa-route-chain-'))
  const app = path.join(root, 'app')
  mkdirSync(app, { recursive: true })
  for (const file of tree) {
    const target = path.join(app, file)
    mkdirSync(path.dirname(target), { recursive: true })
    writeFileSync(target, 'export default function Fixture() {}')
  }
  return { root, app }
}

describe('route chain discovery', () => {
  for (const testCase of contract.cases) {
    it(testCase.name, () => {
      const { root, app } = materialise(testCase.tree)
      try {
        const routeDir = testCase.routeDir === '' ? app : path.join(app, testCase.routeDir)
        const relative = (files) =>
          files.map((file) => path.relative(app, file).replaceAll('\\', '/'))

        assert.deepEqual(relative(collectLayouts(app, routeDir)), testCase.layouts)
        assert.deepEqual(relative(collectTemplates(app, routeDir)), testCase.templates)
      } finally {
        rmSync(root, { recursive: true, force: true })
      }
    })
  }
})
