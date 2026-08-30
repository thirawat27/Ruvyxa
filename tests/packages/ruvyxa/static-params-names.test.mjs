import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))

/** Every quoted string in the array literal assigned after `marker`. */
function listAfter(source, marker) {
  const start = source.indexOf(marker)
  assert.notEqual(start, -1, `${marker} is missing`)
  // Skip a Rust type annotation such as `: [&str; 3]` and read the value.
  const assign = source.indexOf('=', start)
  const open = source.indexOf('[', assign)
  const close = source.indexOf(']', open)
  assert.ok(open !== -1 && close > open, `${marker} is not followed by a list`)
  return [...source.slice(open, close).matchAll(/["']([^"']+)["']/g)].map((match) => match[1])
}

/**
 * Two halves decide what a static parameter set is called: the route graph
 * decides whether a page *has* one, and the worker decides what to call when it
 * does. A name recognised by one and not the other is a route that discovers as
 * SSG and then pre-renders nothing — silently, because neither half is wrong on
 * its own.
 *
 * `generateStaticParams` is Next.js's name for the same export with the same
 * contract, and it is accepted so a page brought over from Next.js declares its
 * parameters to something rather than to nothing.
 */
describe('static params export names', () => {
  it('are the same list in the route graph and the worker', async () => {
    const rust = await readFile(
      path.join(workspaceRoot, 'crates/ruvyxa_graph/src/exports.rs'),
      'utf8',
    )
    const worker = await readFile(
      path.join(workspaceRoot, 'packages/ruvyxa/runtime/worker-pool.mjs'),
      'utf8',
    )

    const discovered = listAfter(rust, 'pub const STATIC_PARAMS_EXPORTS')
    const resolved = listAfter(worker, 'const STATIC_PARAMS_EXPORTS =')

    assert.deepEqual(
      resolved,
      discovered,
      'the graph and the worker must recognise the same export names, in the same order',
    )
    assert.ok(
      discovered.includes('generateStaticParams'),
      "Next.js's name for this export is accepted on purpose",
    )
    assert.ok(discovered.includes('getStaticParams'), "Ruvyxa's own name must keep working")
  })
})
