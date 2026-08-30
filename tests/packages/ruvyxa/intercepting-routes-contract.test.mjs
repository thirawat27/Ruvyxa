/**
 * The dev server's half of `tests/fixtures/intercepting-route-conformance.json`.
 *
 * An intercepting route is discovered twice: `route_intercepts()` in
 * `crates/ruvyxa_graph/src/parallel.rs` answers for `ruvyxa build`, and
 * `collectIntercepts()` in `packages/ruvyxa/runtime/route-intercepts.mjs` answers
 * for `ruvyxa dev`. An interception one host composes and the other does not is
 * a modal that opens in production and does nothing locally.
 *
 * The Rust half is `interception_discovery_matches_the_shared_conformance_table`.
 */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const modulePath = path.join(workspaceRoot, 'packages/ruvyxa/runtime/route-intercepts.mjs')

const { collectIntercepts } = await import(`file://${modulePath.replaceAll('\\', '/')}`)

const contract = JSON.parse(
  readFileSync(
    path.join(workspaceRoot, 'tests/fixtures/intercepting-route-conformance.json'),
    'utf8',
  ),
)

function materialise(tree) {
  const root = mkdtempSync(path.join(os.tmpdir(), 'ruvyxa-intercepts-'))
  const app = path.join(root, 'app')
  for (const file of tree) {
    const target = path.join(app, file)
    mkdirSync(path.dirname(target), { recursive: true })
    writeFileSync(target, 'export default function Fixture() {}')
  }
  return { root, app }
}

describe('intercepting route discovery', () => {
  for (const testCase of contract.cases) {
    it(testCase.name, () => {
      const { root, app } = materialise(testCase.tree)
      try {
        const routeDir = testCase.routeDir === '' ? app : path.join(app, testCase.routeDir)
        const actual = collectIntercepts(app, routeDir).map((intercept) => ({
          level: intercept.levelId,
          name: intercept.name,
          target: intercept.target,
          file: path.relative(app, intercept.file).replaceAll('\\', '/'),
        }))
        assert.deepEqual(actual, testCase.intercepts)
      } finally {
        rmSync(root, { recursive: true, force: true })
      }
    })
  }

  it('skips a marker that climbs above the app root', () => {
    // Route discovery refuses this tree with RUV1018 before the dev server
    // ever builds an entry from it, so the worker never sees one. Skipping is
    // the safe direction: an interception that resolves to nothing is a modal
    // that does not open, where a wrong target would overlay the wrong page.
    const { root, app } = materialise(['photo/page.tsx', '@modal/(..)photo/page.tsx'])
    try {
      assert.deepEqual(collectIntercepts(app, app), [])
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })

  it('reports a directory it could not read rather than finding no interception', () => {
    // The other half of the same rule. `route_intercepts` in
    // `crates/ruvyxa_graph/src/parallel.rs` refuses an unreadable directory
    // with RUV1021, because a walk that could not look has not established
    // that there is nothing there — and an interception that disappears
    // silently is a modal that opens under one host and not the other. A
    // directory that is not there is the portable stand-in for one that cannot
    // be read; a permission bit is not.
    const { root, app } = materialise(['photo/page.tsx'])
    try {
      assert.throws(() => collectIntercepts(app, path.join(app, 'missing')), /RUV1021/)
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })
})
