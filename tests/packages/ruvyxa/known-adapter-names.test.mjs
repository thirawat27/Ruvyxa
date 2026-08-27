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
  // Skip a Rust type annotation such as `: [&str; 11]` and read the value.
  const assign = source.indexOf('=', start)
  const open = source.indexOf('[', assign)
  const close = source.indexOf(']', open)
  assert.ok(open !== -1 && close > open, `${marker} is not followed by a list`)
  return [...source.slice(open, close).matchAll(/["']([^"']+)["']/g)].map((match) => match[1])
}

/**
 * Two halves decide what an adapter is called: the CLI validates
 * `--adapter <name>` against one list, and the adapter runner resolves a name
 * to a package against the other. A name in one and not the other is either a
 * flag that passes validation and then builds nothing, or an adapter that
 * exists and the flag refuses — and neither half is wrong on its own, so
 * neither reports anything useful.
 *
 * Ordered, because the lists are read by people as well as by code and a
 * reordering is a diff worth seeing.
 *
 * Registered in `scripts/check-cross-language-constants.mjs` as what holds this
 * pair; that check fails if this file stops existing.
 */
describe('known adapter names', () => {
  it('are the same list in the CLI and the adapter runner', async () => {
    const cli = await readFile(path.join(workspaceRoot, 'crates/ruvyxa_cli/src/main.rs'), 'utf8')
    const runner = await readFile(
      path.join(workspaceRoot, 'packages/ruvyxa/runtime/adapter-runner.mjs'),
      'utf8',
    )

    const validated = listAfter(cli, 'const KNOWN_ADAPTER_NAMES')
    const resolved = listAfter(runner, 'const KNOWN_ADAPTER_NAMES =')

    assert.ok(validated.length > 0, 'the CLI must know at least one adapter')
    assert.deepEqual(
      resolved,
      validated,
      'the CLI and the adapter runner must know the same adapter names, in the same order',
    )
  })

  it('each name a `--adapter` flag accepts has a package to resolve to', async () => {
    const cli = await readFile(path.join(workspaceRoot, 'crates/ruvyxa_cli/src/main.rs'), 'utf8')
    for (const name of listAfter(cli, 'const KNOWN_ADAPTER_NAMES')) {
      const manifest = path.join(workspaceRoot, `packages/@ruvyxa/adapter-${name}/package.json`)
      const { name: packageName } = JSON.parse(await readFile(manifest, 'utf8'))
      assert.equal(
        packageName,
        `@ruvyxa/adapter-${name}`,
        `--adapter ${name} must name a package that exists`,
      )
    }
  })
})
