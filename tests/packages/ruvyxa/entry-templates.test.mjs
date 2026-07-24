import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

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
      /React\.createElement\(__ruvyxaBoundary, \{ errorFallback: RouteError, notFound: RouteNotFound \}, tree\)/,
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
