/**
 * Which of this package's modules are client components, and which are not.
 *
 * A server component may import `@ruvyxa/react` — a root layout that renders a
 * `<Link>` nav is the ordinary case — and the react-server build of React has
 * no `Component` to extend, no `createContext`, and no effects. Without the
 * directive the server-components graph compiled those modules for real and
 * the whole render died with `Class extends value undefined`, so the boundary
 * below is load-bearing rather than decorative.
 *
 * Asserted against `dist`, not `src`: the directive only does anything if it
 * survives `tsc`, and it is the built file every consumer resolves.
 */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const distDir = path.resolve(fileURLToPath(new URL('../dist', import.meta.url)))

/** Modules that must be references in a server-components graph. */
const CLIENT_MODULES = [
  // Hooks, refs, and a click handler that drives the router.
  'link.js',
  // A class component, which the react-server build cannot even extend.
  'error-boundary.js',
  // `createContext`, `useContext`, `useSyncExternalStore`.
  'route-context.js',
  // An effect that inserts and removes a live script tag.
  'script.js',
  // State and effects over a loader's lifecycle.
  'use-loader.js',
]

/**
 * Modules that must *not* carry the directive.
 *
 * Marking one of these would push work a server component can do today into
 * the browser bundle for no reason — `Seo` and `Image` render markup and read
 * no React state at all.
 */
const SERVER_SAFE_MODULES = ['image.js', 'seo.js', 'not-found.js', 'route-types.js', 'index.js']

async function directive(file) {
  const source = await readFile(path.join(distDir, file), 'utf8')
  const first = source.trimStart().split('\n', 1)[0].trim()
  return first === `'use client';` || first === `"use client";` ? 'use client' : null
}

describe('the package client boundary', () => {
  for (const file of CLIENT_MODULES) {
    it(`marks ${file} as a client module`, async () => {
      assert.equal(
        await directive(file),
        'use client',
        `${file} must open with 'use client' in dist/, before any other statement`,
      )
    })
  }

  for (const file of SERVER_SAFE_MODULES) {
    it(`leaves ${file} usable from a server component`, async () => {
      assert.equal(await directive(file), null, `${file} must not declare a client boundary`)
    })
  }

  it('keeps the barrel itself server-side so it can re-export both halves', async () => {
    // The barrel is walked by the server graph; each `'use client'` module it
    // re-exports becomes a reference on its own. Marking the barrel would turn
    // every export in this package into one, including the components a server
    // component is meant to render itself.
    assert.equal(await directive('index.js'), null)
    const source = await readFile(path.join(distDir, 'index.js'), 'utf8')
    for (const file of CLIENT_MODULES) {
      assert.match(source, new RegExp(`from ['"]\\./${file.replace('.js', '\\.js')}['"]`), file)
    }
  })
})
