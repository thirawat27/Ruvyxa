import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import os from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { pathToFileURL } from 'node:url'

import {
  clientEntrySource,
  needsRouteBoundary,
  nodeSsrEntrySource,
  routeBoundaryPrelude,
  routeContextPrelude,
  routeRegistration,
  routeTreeFunction,
} from '../../../packages/ruvyxa/runtime/entry-templates.mjs'

describe('entry-templates route composition', () => {
  it('binds the shared routing context on globalThis, not an import', () => {
    // A generated entry cannot depend on @ruvyxa/react: an app may render plain
    // React pages and never install it. Both sides meet on globalThis.
    const prelude = routeContextPrelude()
    assert.match(prelude, /globalThis\.__RUVYXA_ROUTE_CONTEXT__ \|\|= React\.createContext\(null\)/)
    assert.doesNotMatch(prelude, /import/)
  })

  it('wraps the page and layouts in the routing context provider', () => {
    const tree = routeTreeFunction({
      name: '__ruvyxaTree',
      pageName: 'Page',
      layoutNames: ['Layout0', 'Layout1'],
      routePath: '/blog/[slug]',
    })
    assert.match(
      tree,
      /React\.createElement\(Page, \{ params: ctx\.params \?\? \{\}, requestPath: ctx\.path \}\)/,
    )
    assert.match(tree, /\[Layout0, Layout1\]\.reverse\(\)/)
    assert.match(tree, /__ruvyxaRouteContext\.Provider/)
    assert.match(tree, /route: "\/blog\/\[slug\]"/)
  })

  it('escapes a route path that contains a quote', () => {
    // The route pattern is interpolated as a JS string literal; an unescaped
    // quote would close it early and inject code.
    const tree = routeTreeFunction({
      name: 't',
      pageName: 'Page',
      layoutNames: [],
      routePath: '/a";globalThis.pwned=1;"',
    })
    assert.doesNotMatch(tree, /globalThis\.pwned=1;"\s*\}/)
    assert.match(tree, /route: "\/a\\";globalThis\.pwned=1;\\""/)
  })

  it('registers the route so the client router can re-render it', () => {
    const registration = routeRegistration({ name: '__ruvyxaTree', routePath: '/about' })
    assert.match(registration, /globalThis\.__RUVYXA_ROUTES__ \|\|= \{\}/)
    assert.match(registration, /\["\/about"\] = __ruvyxaTree/)
  })

  it('publishes the route pattern next to the registry entry', () => {
    // The registry is keyed by pattern, so the client router needs the pattern
    // to look up the route the document was served from. Without it the router
    // seeded its snapshot from the concrete URL and `router.refresh()` silently
    // rendered nothing on every dynamic route.
    const registration = routeRegistration({
      name: '__ruvyxaTree',
      routePath: '/blog/[slug]',
    })
    assert.match(registration, /globalThis\.__RUVYXA_ROUTE_PATTERN__ = "\/blog\/\[slug\]"/)
  })

  it('escapes the route pattern in the published global', () => {
    const registration = routeRegistration({
      name: '__ruvyxaTree',
      routePath: '/a";globalThis.pwned=1;"',
    })
    assert.match(registration, /__RUVYXA_ROUTE_PATTERN__ = "\/a\\";globalThis\.pwned=1;\\""/)
  })

  it('client entry hydrates into an existing root or creates one', () => {
    const source = clientEntrySource({
      imports: ['import Page from "./page.js"'],
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
      requestPathLiteral: '"/"',
      paramsLiteral: '{}',
    })
    assert.match(source, /hydrateRoot\(document, __ruvyxaTreeElement\)/)
    assert.match(source, /globalThis\.__RUVYXA_ROOT__\.render\(__ruvyxaTreeElement\)/)
    assert.match(source, /\(globalThis\.__RUVYXA_ROUTES__ \|\|= \{\}\)\["\/"\] = __ruvyxaTree/)
  })

  it('server entry provides the routing context and no client registry', () => {
    const source = nodeSsrEntrySource({
      imports: ['import Page from "./page.js"'],
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
    })
    assert.match(source, /__ruvyxaRouteContext\.Provider/)
    // There is no root to render into on the server, and the global would leak
    // across requests in a long-lived worker.
    assert.doesNotMatch(source, /__RUVYXA_ROUTES__/)
    assert.match(source, /renderToPipeableStream/)
  })

  it('exports Flight only when a page module namespace is supplied', () => {
    const plain = nodeSsrEntrySource({
      imports: ['import Page from "./page.js"'],
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
    })
    const flight = nodeSsrEntrySource({
      imports: ['import Page, * as PageModule from "./page.js"'],
      pageName: 'Page',
      pageModuleName: 'PageModule',
      layoutNames: [],
      routePath: '/',
    })

    assert.doesNotMatch(plain, /export async function flight/)
    assert.match(flight, /export async function flight\(ctx\)/)
    assert.match(flight, /return PageModule\.flight\(ctx\)/)
  })

  it('partial-prerender mode commits the shell early and tolerates slot errors', () => {
    const ppr = nodeSsrEntrySource({
      imports: [],
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
      readyEvent: 'onShellReady',
      tolerateStreamErrors: true,
    })
    assert.match(ppr, /onShellReady\(\)/)
    assert.doesNotMatch(ppr, /onShellReady\(\)[\s\S]*reject\(error\)[\s\S]*onShellError/)
  })

  it('renders an async component when react-dom/server has only the web renderer', async () => {
    // Bun and Deno resolve `react-dom/server` to an entry point that exports
    // `renderToReadableStream` and no `renderToPipeableStream`. The legacy
    // `renderToString` this entry used to fall back to is synchronous: a
    // component that awaits anything makes it throw "A component suspended
    // while responding to synchronous input" instead of rendering, which is
    // every async server component on those two runtimes.
    const dir = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-web-stream-'))
    try {
      const require = createRequire(import.meta.url)
      const reactUrl = pathToFileURL(require.resolve('react')).href
      const serverUrl = pathToFileURL(require.resolve('react-dom/server')).href

      // The shape Bun and Deno hand the entry: the web renderer and the legacy
      // string renderer, with no pipeable one to prefer.
      await writeFile(
        path.join(dir, 'web-server.mjs'),
        `import * as server from ${JSON.stringify(serverUrl)}\n` +
          `export const renderToReadableStream = server.renderToReadableStream\n` +
          `export const renderToString = server.renderToString\n`,
      )
      await writeFile(
        path.join(dir, 'page.mjs'),
        `import React from ${JSON.stringify(reactUrl)}\n` +
          `export default async function Page() {\n` +
          `  const note = await Promise.resolve("awaited on the server")\n` +
          `  return React.createElement("html", null, React.createElement("body", null, note))\n` +
          `}\n`,
      )
      const source = nodeSsrEntrySource({
        imports: ['import Page from "./page.mjs"'],
        pageName: 'Page',
        layoutNames: [],
        routePath: '/',
      })
        .replace('"react"', JSON.stringify(reactUrl))
        .replace('"react-dom/server"', '"./web-server.mjs"')
      await writeFile(path.join(dir, 'entry.mjs'), source)

      const { render } = await import(pathToFileURL(path.join(dir, 'entry.mjs')).href)
      const html = await render({ path: '/', params: {} })

      assert.match(html, /^<!doctype html>/i)
      assert.match(html, /awaited on the server/)
      // Nothing was read until the render finished, so the document holds the
      // finished markup rather than a fallback and the script that replaces it.
      assert.doesNotMatch(html, /\$RC/)
    } finally {
      await rm(dir, { recursive: true, force: true })
    }
  })
})

describe('entry-templates special files', () => {
  it('wraps the page in the boundary, then Suspense, then layouts', () => {
    const tree = routeTreeFunction({
      name: '__ruvyxaTree',
      pageName: 'Page',
      layoutNames: ['Layout0'],
      routePath: '/blog/[slug]',
      errorName: 'RouteError',
      loadingName: 'RouteLoading',
      notFoundName: 'RouteNotFound',
    })
    assert.match(
      tree,
      /React\.createElement\(__ruvyxaBoundary, \{ errorFallback: RouteError, notFound: RouteNotFound, defaultErrorFallback: false \}, tree\)/,
    )
    assert.match(
      tree,
      /React\.createElement\(React\.Suspense, \{ fallback: React\.createElement\(RouteLoading, null\) \}, tree\)/,
    )
    // The boundary must be inner (applied first) so a synchronous throw is caught
    // before it reaches the Suspense and turns into a loading flash on the server.
    assert.ok(tree.indexOf('__ruvyxaBoundary') < tree.indexOf('React.Suspense'), tree)
    // Layouts still wrap both, so a layout persists while its page loads/fails.
    assert.ok(tree.indexOf('React.Suspense') < tree.indexOf('[Layout0].reverse()'), tree)
  })

  it('passes null for an absent fallback so the boundary can rethrow', () => {
    const tree = routeTreeFunction({
      name: 't',
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
      notFoundName: 'RouteNotFound',
    })
    assert.match(tree, /errorFallback: null, notFound: RouteNotFound/)
  })

  it('emits neither Suspense nor boundary when a route has no specials', () => {
    const tree = routeTreeFunction({
      name: 't',
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
    })
    assert.doesNotMatch(tree, /React\.Suspense/)
    assert.doesNotMatch(tree, /__ruvyxaBoundary/)
  })

  it('distinguishes not-found from other errors by the notFound marker', () => {
    const prelude = routeBoundaryPrelude()
    assert.match(prelude, /class __ruvyxaBoundary extends React\.Component/)
    assert.match(prelude, /error\.__ruvyxaNotFound/)
    assert.match(prelude, /this\.props\.notFound/)
    assert.match(prelude, /this\.props\.errorFallback/)
    // A boundary with no matching fallback rethrows so an ancestor can handle it.
    assert.match(prelude, /throw error/)
  })

  /**
   * The prelude is assembled as a template string, so nothing in the normal
   * build parses what comes out of it — a stray backtick anywhere inside,
   * including in a comment, closes the string early and emits a class that
   * cannot compile. That has now happened twice in this repository, in two
   * different generators, and both times it surfaced only when a project was
   * built. Compiling the emitted class is the cheap guard.
   */
  it('emits a boundary class that actually compiles', () => {
    const prelude = routeBoundaryPrelude()
    // `extends React.Component` is evaluated when the class is defined, so a
    // stand-in has to be in scope. Only the shape the prelude touches matters.
    const React = {
      Component: class {
        constructor(props) {
          this.props = props
          this.state = {}
        }
        setState(next) {
          this.state = { ...this.state, ...next }
        }
      },
      createElement: () => null,
    }
    const Boundary = new Function('React', `${prelude}; return __ruvyxaBoundary`)(React)
    assert.equal(typeof Boundary, 'function')
    assert.equal(typeof Boundary.getDerivedStateFromError, 'function')
    assert.deepEqual(Boundary.getDerivedStateFromError('boom'), { error: 'boom' })

    // Both recovery paths must exist on an instance, not just in the source.
    const instance = new Boundary({})
    assert.equal(typeof instance.reset, 'function')
    assert.equal(typeof instance.retry, 'function')
    // With no router mounted, retry degrades to a reset rather than throwing.
    instance.setState({ error: 'boom' })
    return instance.retry().then(() => {
      assert.equal(instance.state.error, null)
    })
  })

  /**
   * `reset` clears the boundary's own state, so it can only recover from a
   * fault in the client tree. A page whose server data failed needs the request
   * repeated, which is what `retry` asks the router to do — and when no router
   * is mounted there is nothing to re-fetch from, so it degrades to a reset
   * rather than silently doing nothing.
   */
  it('offers the fallback a server-backed retry as well as a local reset', () => {
    const prelude = routeBoundaryPrelude()
    assert.match(prelude, /reset: this\.reset/)
    assert.match(prelude, /retry: this\.retry/)
    assert.match(prelude, /__RUVYXA_ROUTER_INSTANCE__/)
    assert.match(prelude, /typeof router\.retry !== "function"/)
  })

  /**
   * The shell is the half of a route that needs no server data: its layouts
   * wrapped around `loading.tsx`, both already in the route bundle. Painting it
   * is what lets a navigation show the destination immediately instead of
   * leaving the previous page on screen until the Flight payload lands.
   */
  it('emits a loading shell the client router can paint without server data', () => {
    const source = clientEntrySource({
      imports: [],
      pageName: 'Page',
      layoutNames: ['Layout0'],
      routePath: '/blog/[slug]',
      requestPathLiteral: '"/blog/x"',
      paramsLiteral: '{}',
      loadingName: 'RouteLoading',
    })

    assert.match(source, /function __ruvyxaShell\(ctx\)/)
    assert.match(source, /__RUVYXA_SHELLS__ \|\|= \{\}\)\["\/blog\/\[slug\]"\] = __ruvyxaShell/)
    // The loading component sits inside the layouts, with no page.
    assert.match(source, /let tree = React\.createElement\(RouteLoading, null\)/)
    // A stale payload from the page being navigated away from must never reach
    // the shell, so the context carries no flight value at all.
    const shell = source.slice(source.indexOf('function __ruvyxaShell'))
    assert.match(shell, /flight: undefined/)
    assert.doesNotMatch(shell.slice(0, shell.indexOf('__RUVYXA_SHELLS__')), /createElement\(Page/)
  })

  it('omits the shell for a route that declares no loading state', () => {
    const source = clientEntrySource({
      imports: [],
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
      requestPathLiteral: '"/"',
      paramsLiteral: '{}',
    })
    assert.doesNotMatch(source, /__ruvyxaShell/)
    assert.doesNotMatch(source, /__RUVYXA_SHELLS__/)
  })

  it('needs the boundary only for error/not-found, not loading alone', () => {
    assert.equal(needsRouteBoundary({ errorName: 'E' }), true)
    assert.equal(needsRouteBoundary({ notFoundName: 'N' }), true)
    assert.equal(needsRouteBoundary({ loadingName: 'L' }), false)
    assert.equal(needsRouteBoundary({}), false)
  })

  it('includes the boundary class in a client entry that needs it', () => {
    const withBoundary = clientEntrySource({
      imports: ['import Page from "./page.js"', 'import RouteError from "./error.js"'],
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
      requestPathLiteral: '"/"',
      paramsLiteral: '{}',
      errorName: 'RouteError',
    })
    assert.match(withBoundary, /class __ruvyxaBoundary extends React\.Component/)

    const withoutBoundary = clientEntrySource({
      imports: ['import Page from "./page.js"'],
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
      requestPathLiteral: '"/"',
      paramsLiteral: '{}',
    })
    assert.doesNotMatch(withoutBoundary, /__ruvyxaBoundary/)
  })
})
