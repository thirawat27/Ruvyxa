/**
 * No exported name may mean two different things depending on where it is
 * imported from.
 *
 * `notFound` did. `@ruvyxa/core/server` exported one that *returned* a 404
 * `Response`; `@ruvyxa/react` exported one that *threw* a tagged signal for the
 * route boundary to turn into `not-found.tsx`. Both were public, both were
 * documented, and the docs had to carry a "do not confuse it with" note plus a
 * troubleshooting entry — which is what a collision costs once it ships.
 *
 * Nothing failed at the import. A page that took the server half rendered a
 * `Response` object where React expected an element; an API route that took the
 * browser half threw instead of answering. The type checker cannot help,
 * because both names type-check at their own call site.
 *
 * So the rule is asserted over the built entry points rather than left to a
 * paragraph in chapter 4: a name that reaches both the server surface and the
 * browser surface is a name that will be imported from the wrong one.
 */
import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, existsSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))

/**
 * The two halves, by the package that owns them.
 *
 * `ruvyxa` is omitted deliberately: it is one line, `export * from
 * '@ruvyxa/core'`, so it carries exactly the core surface and listing it would
 * report every core name as its own duplicate.
 */
const SERVER_PACKAGES = ['packages/@ruvyxa/core'] as const
const BROWSER_PACKAGES = ['packages/@ruvyxa/react'] as const

/** Runtime values a built entry point exports, by entry specifier. */
async function surfaceOf(packageDir: string): Promise<Map<string, Set<string>>> {
  const manifestPath = path.join(repoRoot, packageDir, 'package.json')
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as {
    name: string
    exports?: Record<string, { default?: string } | string>
  }
  const surfaces = new Map<string, Set<string>>()
  for (const [entry, target] of Object.entries(manifest.exports ?? {})) {
    const file = typeof target === 'string' ? target : target.default
    if (!file) continue
    const absolute = path.join(repoRoot, packageDir, file)
    // A missing build is a broken test run, not a passing one: an entry point
    // that cannot be loaded would silently contribute no names and let a real
    // collision through.
    assert.ok(
      existsSync(absolute),
      `${manifest.name}${entry.slice(1)} is not built (${file}); run \`pnpm -r build\` first`,
    )
    const loaded = (await import(`file://${absolute.replaceAll('\\', '/')}`)) as Record<
      string,
      unknown
    >
    const specifier = `${manifest.name}${entry === '.' ? '' : entry.slice(1)}`
    surfaces.set(
      specifier,
      new Set(Object.keys(loaded).filter((name) => name !== 'default' && name !== '__esModule')),
    )
  }
  return surfaces
}

describe('public entry points', () => {
  it('never give one name to both a server export and a browser export', async () => {
    const server = new Map<string, string[]>()
    for (const dir of SERVER_PACKAGES) {
      for (const [specifier, names] of await surfaceOf(dir)) {
        for (const name of names) {
          server.set(name, [...(server.get(name) ?? []), specifier])
        }
      }
    }

    const collisions: string[] = []
    for (const dir of BROWSER_PACKAGES) {
      for (const [specifier, names] of await surfaceOf(dir)) {
        for (const name of names) {
          const alsoServer = server.get(name)
          if (alsoServer) {
            collisions.push(`${name}: ${specifier}  and  ${alsoServer.join(', ')}`)
          }
        }
      }
    }

    assert.deepEqual(
      collisions,
      [],
      `one name, two meanings — rename the one whose behaviour is less obvious from the name:
  ${collisions.join('\n  ')}`,
    )
  })

  it('keeps the two not-found paths on names that cannot be confused', async () => {
    const core = await import(
      `file://${path.join(repoRoot, 'packages/@ruvyxa/core/dist/server.js').replaceAll('\\', '/')}`
    )
    const react = await import(
      `file://${path.join(repoRoot, 'packages/@ruvyxa/react/dist/index.js').replaceAll('\\', '/')}`
    )

    // The server half returns a Response, and is no longer 404-only.
    const response = core.status(404, 'gone')
    assert.ok(response instanceof Response)
    assert.equal(response.status, 404)
    assert.equal(await response.text(), 'gone')
    assert.equal(core.notFound, undefined, 'the colliding name must not come back')
    assert.equal(core.notFoundResponse, undefined, 'the interim name was replaced by status()')

    // The browser half throws, and is the one that matches Next.js.
    assert.throws(
      () => react.notFound(),
      (error: unknown) => react.isNotFoundError(error),
    )
    assert.equal(react.status, undefined, 'the response helper belongs to the server half only')
  })
})
