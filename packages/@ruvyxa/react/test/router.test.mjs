import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { getRouterInstance } from '../dist/router.js'

function deferred() {
  let resolve
  const promise = new Promise((complete) => {
    resolve = complete
  })
  return { promise, resolve }
}

function routeModuleSource(route, gate) {
  const source = `
    await globalThis[${JSON.stringify(gate)}]
    globalThis.__RUVYXA_ROUTES__[${JSON.stringify(route)}] = (context) => context
  `
  return `data:text/javascript,${encodeURIComponent(source)}`
}

/**
 * Minimal `document`/`CSS` stand-ins covering exactly what `prefetch` touches:
 * an href-keyed `modulepreload` lookup and an appendable head. A real DOM is
 * not needed to observe how many hints the router emits.
 */
function stubPreloadDocument() {
  const links = []
  return {
    links,
    document: {
      head: {
        append(node) {
          links.push(node)
        },
      },
      createElement() {
        return { rel: '', href: '' }
      },
      querySelector(selector) {
        const match = /^link\[rel="modulepreload"\]\[href="(.*)"\]$/.exec(selector)
        if (!match) return null
        const href = match[1].replaceAll('\\', '')
        return links.find((link) => link.rel === 'modulepreload' && link.href === href) ?? null
      },
    },
  }
}

/** Install a minimal browser-ish global environment and restore it after. */
function withGlobals(values, run) {
  const keys = [
    'window',
    'document',
    'CSS',
    '__RUVYXA_ROUTES__',
    '__RUVYXA_ROOT__',
    '__RUVYXA_ROUTE_MANIFEST__',
    '__RUVYXA_ROUTE_PARAMS__',
    '__RUVYXA_REQUEST_PATH__',
    '__RUVYXA_ROUTE_PATTERN__',
    '__RUVYXA_ROUTE_ARTIFACTS__',
    '__RUVYXA_ROUTER_INSTANCE__',
    '__RUVYXA_INTERCEPTS__',
  ]
  const previous = new Map(
    keys.map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]),
  )
  for (const key of keys) delete globalThis[key]
  Object.assign(globalThis, values)
  const restore = () => {
    for (const key of keys) delete globalThis[key]
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor)
    }
  }
  try {
    const result = run()
    if (result && typeof result.then === 'function') return result.finally(restore)
    restore()
    return result
  } catch (error) {
    restore()
    throw error
  }
}

function browserWindow(pathname) {
  return {
    location: {
      href: `https://example.test${pathname}`,
      origin: 'https://example.test',
      pathname,
      search: '',
      assign() {},
      replace() {},
    },
    history: { pushState() {}, replaceState() {}, back() {}, forward() {} },
    addEventListener() {},
    scrollTo() {},
  }
}

describe('client router initial route', () => {
  it('seeds the route from the published pattern so refresh works on a dynamic route', () => {
    // Regression: the snapshot's `route` used to come from
    // `__RUVYXA_REQUEST_PATH__` (a concrete URL), so looking the route up in
    // `__RUVYXA_ROUTES__` — which is keyed by pattern — always missed, and
    // `refresh()` rendered nothing without reporting anything.
    const rendered = []
    withGlobals(
      {
        window: browserWindow('/blog/hello'),
        __RUVYXA_REQUEST_PATH__: '/blog/hello',
        __RUVYXA_ROUTE_PATTERN__: '/blog/[slug]',
        __RUVYXA_ROUTE_PARAMS__: { slug: 'hello' },
        __RUVYXA_ROUTES__: {
          '/blog/[slug]': (context) => ({ tree: context.route }),
        },
        __RUVYXA_ROOT__: {
          render(tree) {
            rendered.push(tree)
          },
        },
      },
      () => {
        const router = getRouterInstance()

        assert.equal(router.getSnapshot().route, '/blog/[slug]')
        assert.equal(router.getSnapshot().pathname, '/blog/hello')
        assert.deepEqual(router.getSnapshot().params, { slug: 'hello' })

        router.refresh()
        assert.deepEqual(rendered, [{ tree: '/blog/[slug]' }])
      },
    )
  })

  it('falls back to matching the manifest when no pattern global is present', () => {
    // Keeps a document built by an older bundle working.
    withGlobals(
      {
        window: browserWindow('/blog/hello'),
        __RUVYXA_REQUEST_PATH__: '/blog/hello',
        __RUVYXA_ROUTE_MANIFEST__: {
          routes: [{ path: '/blog/[slug]', src: '/chunks/blog.js' }],
        },
      },
      () => {
        assert.equal(getRouterInstance().getSnapshot().route, '/blog/[slug]')
      },
    )
  })

  it('reports a refresh that cannot find its bundle instead of doing nothing', () => {
    withGlobals(
      {
        window: browserWindow('/blog/hello'),
        __RUVYXA_REQUEST_PATH__: '/blog/hello',
        __RUVYXA_ROUTE_PATTERN__: '/blog/[slug]',
        __RUVYXA_ROUTES__: {},
        __RUVYXA_ROOT__: { render() {} },
      },
      () => {
        assert.throws(() => getRouterInstance().refresh(), /not registered/)
      },
    )
  })
})

describe('client router prefetch hints', () => {
  it('emits one modulepreload per module when routes share a chunk', async () => {
    const keys = [
      'window',
      'document',
      'CSS',
      '__RUVYXA_ROUTES__',
      '__RUVYXA_ROUTE_MANIFEST__',
      '__RUVYXA_ROUTER_INSTANCE__',
    ]
    const previous = new Map(
      keys.map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]),
    )

    try {
      const preload = stubPreloadDocument()
      globalThis.window = {
        location: {
          href: 'https://example.test/',
          origin: 'https://example.test',
          pathname: '/',
          search: '',
          assign() {},
          replace() {},
        },
        history: { pushState() {}, replaceState() {}, back() {}, forward() {} },
        addEventListener() {},
        scrollTo() {},
      }
      globalThis.document = preload.document
      globalThis.CSS = { escape: (value) => value }
      globalThis.__RUVYXA_ROUTES__ = {}
      globalThis.__RUVYXA_ROUTE_MANIFEST__ = {
        routes: [
          { path: '/a', src: '/chunks/a.js', sharedChunks: [{ src: '/chunks/vendor.js' }] },
          { path: '/b', src: '/chunks/b.js', sharedChunks: [{ src: '/chunks/vendor.js' }] },
        ],
      }
      delete globalThis.__RUVYXA_ROUTER_INSTANCE__

      const router = getRouterInstance()
      // `prefetch` resolves the manifest first, so each call finishes its work
      // in a microtask rather than inline.
      router.prefetch('/a')
      await Promise.resolve()
      router.prefetch('/b')
      await Promise.resolve()
      // A repeat of an already hinted route must stay a no-op.
      router.prefetch('/a')
      await Promise.resolve()

      const hinted = preload.links.map((link) => link.href)
      assert.deepEqual([...hinted].sort(), ['/chunks/a.js', '/chunks/b.js', '/chunks/vendor.js'])
    } finally {
      for (const [key, descriptor] of previous) {
        if (descriptor) Object.defineProperty(globalThis, key, descriptor)
        else delete globalThis[key]
      }
    }
  })
})

describe('client router navigation state', () => {
  it('falls back to a document navigation when a cached route artifact is stale', async () => {
    const assigned = []
    await withGlobals(
      {
        window: {
          ...browserWindow('/'),
          location: {
            ...browserWindow('/').location,
            assign: (href) => assigned.push(href),
          },
        },
        __RUVYXA_ROUTES__: { '/stale': () => null },
        __RUVYXA_ROUTE_ARTIFACTS__: { '/stale': 'old' },
        __RUVYXA_ROOT__: { render() {} },
        __RUVYXA_ROUTE_MANIFEST__: {
          routes: [{ path: '/stale', src: '/chunks/stale.js', artifactVersion: 'current' }],
        },
      },
      async () => {
        const router = getRouterInstance()
        await router.navigate('/stale')
        assert.equal(assigned.length, 1)
      },
    )
  })

  it('keeps pending true when a stale route load finishes before the current navigation', async () => {
    const keys = [
      'window',
      '__RUVYXA_ROUTES__',
      '__RUVYXA_ROOT__',
      '__RUVYXA_ROUTE_PARAMS__',
      '__RUVYXA_REQUEST_PATH__',
      '__RUVYXA_ROUTE_MANIFEST__',
      '__RUVYXA_ROUTER_INSTANCE__',
      '__RUVYXA_TEST_ROUTE_A__',
      '__RUVYXA_TEST_ROUTE_B__',
    ]
    const previous = new Map(
      keys.map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]),
    )
    const routeA = deferred()
    const routeB = deferred()

    try {
      globalThis.window = {
        location: {
          href: 'https://example.test/',
          origin: 'https://example.test',
          pathname: '/',
          search: '',
          assign() {},
          replace() {},
        },
        history: {
          pushState() {},
          replaceState() {},
          back() {},
          forward() {},
        },
        addEventListener() {},
        scrollTo() {},
      }
      globalThis.__RUVYXA_ROUTES__ = {}
      globalThis.__RUVYXA_ROOT__ = { render() {} }
      globalThis.__RUVYXA_REQUEST_PATH__ = '/'
      globalThis.__RUVYXA_ROUTE_MANIFEST__ = {
        routes: [
          {
            path: '/slow-a',
            src: routeModuleSource('/slow-a', '__RUVYXA_TEST_ROUTE_A__'),
          },
          {
            path: '/slow-b',
            src: routeModuleSource('/slow-b', '__RUVYXA_TEST_ROUTE_B__'),
          },
        ],
      }
      globalThis.__RUVYXA_TEST_ROUTE_A__ = routeA.promise
      globalThis.__RUVYXA_TEST_ROUTE_B__ = routeB.promise
      delete globalThis.__RUVYXA_ROUTER_INSTANCE__

      const router = getRouterInstance()
      const firstNavigation = router.navigate('/slow-a')
      await Promise.resolve()
      await Promise.resolve()
      assert.equal(router.getPending(), true)

      const secondNavigation = router.navigate('/slow-b')
      await Promise.resolve()
      await Promise.resolve()
      assert.equal(router.getPending(), true)

      routeA.resolve()
      await firstNavigation
      assert.equal(router.getPending(), true)

      routeB.resolve()
      await secondNavigation
      assert.equal(router.getPending(), false)
      assert.equal(router.getSnapshot().pathname, '/slow-b')
    } finally {
      routeA.resolve()
      routeB.resolve()
      for (const [key, descriptor] of previous) {
        if (descriptor) Object.defineProperty(globalThis, key, descriptor)
        else delete globalThis[key]
      }
    }
  })
})

describe('client router view transitions', () => {
  it('uses the native API only when the navigation opts in', async () => {
    const keys = [
      'window',
      'document',
      '__RUVYXA_ROUTES__',
      '__RUVYXA_ROOT__',
      '__RUVYXA_ROUTE_MANIFEST__',
      '__RUVYXA_ROUTER_INSTANCE__',
    ]
    const previous = new Map(
      keys.map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]),
    )
    const rendered = []
    let transitions = 0
    try {
      globalThis.window = browserWindow('/')
      globalThis.document = {
        startViewTransition(update) {
          transitions += 1
          update()
          return { updateCallbackDone: Promise.resolve() }
        },
      }
      globalThis.__RUVYXA_ROUTE_MANIFEST__ = {
        routes: [{ path: '/about', src: '/chunks/about.js' }],
      }
      globalThis.__RUVYXA_ROUTES__ = { '/about': (context) => context.pathname }
      globalThis.__RUVYXA_ROOT__ = { render: (tree) => rendered.push(tree) }

      const router = getRouterInstance()
      await router.navigate('/about', { viewTransition: true })

      assert.equal(transitions, 1)
      assert.deepEqual(rendered, ['/about'])
    } finally {
      for (const key of keys) delete globalThis[key]
      for (const [key, descriptor] of previous) {
        if (descriptor) Object.defineProperty(globalThis, key, descriptor)
      }
    }
  })
})

describe('intercepting routes', () => {
  /** A gallery that intercepts `/gallery/photo` into its `modal` slot. */
  function galleryGlobals(pathname, rendered, historyLog) {
    const window = browserWindow(pathname)
    window.history = {
      pushState(_state, _title, href) {
        historyLog.push(['push', href])
      },
      replaceState(_state, _title, href) {
        historyLog.push(['replace', href])
      },
      back() {},
      forward() {},
    }
    return {
      window,
      __RUVYXA_REQUEST_PATH__: pathname,
      __RUVYXA_ROUTE_PATTERN__: '/gallery',
      __RUVYXA_ROUTE_PARAMS__: {},
      __RUVYXA_ROUTE_MANIFEST__: {
        routes: [
          { path: '/gallery', src: '/gallery.js' },
          { path: '/gallery/photo', src: '/photo.js' },
          { path: '/settings', src: '/settings.js' },
        ],
      },
      __RUVYXA_INTERCEPTS__: {
        '/gallery': [{ level: 'app/gallery', name: 'modal', target: '/gallery/photo' }],
      },
      __RUVYXA_ROUTES__: {
        '/gallery': (context) => ({ ...context }),
        '/settings': (context) => ({ ...context }),
      },
      __RUVYXA_ROOT__: {
        render(tree) {
          rendered.push(tree)
        },
      },
    }
  }

  it('opens the overlay without leaving the mounted route', async () => {
    // The whole point: the bundle already running answers the navigation, so
    // the page underneath keeps its state and nothing is fetched. The tree is
    // rendered with the *mounted* pathname — `template.tsx` is keyed on it, and
    // a new one would remount the page the overlay sits on.
    const rendered = []
    const history = []
    await withGlobals(galleryGlobals('/gallery', rendered, history), async () => {
      const router = getRouterInstance()
      await router.navigate('/gallery/photo', {})

      assert.equal(rendered.length, 1)
      assert.equal(rendered[0].route, '/gallery', 'the mounted route does not change')
      assert.equal(rendered[0].pathname, '/gallery', 'the tree keeps the mounted pathname')
      assert.deepEqual(rendered[0].intercept, {
        level: 'app/gallery',
        name: 'modal',
        target: '/gallery/photo',
        params: {},
        path: '/gallery/photo',
      })
      assert.deepEqual(history, [['push', 'https://example.test/gallery/photo']])
      // The address bar moved, so anything reading the router agrees with it.
      assert.equal(router.getSnapshot().pathname, '/gallery/photo')
      assert.equal(router.getSnapshot().route, '/gallery')
    })
  })

  it('closes the overlay when the URL goes back to the mounted route', async () => {
    const rendered = []
    const history = []
    await withGlobals(galleryGlobals('/gallery', rendered, history), async () => {
      const router = getRouterInstance()
      await router.navigate('/gallery/photo', {})
      await router.navigate('/gallery', {})

      assert.equal(rendered.length, 2)
      assert.equal(rendered[1].intercept, undefined, 'the overlay is gone')
      assert.equal(rendered[1].route, '/gallery')
    })
  })

  it('does not intercept for a route that publishes no table', async () => {
    // A visitor arriving from anywhere else gets the real page. The overlay
    // lives in the gallery's bundle, so nothing else can render it — which is
    // also why a reload and a shared link show the page rather than the modal.
    const rendered = []
    const history = []
    const globals = galleryGlobals('/settings', rendered, history)
    globals.__RUVYXA_ROUTE_PATTERN__ = '/settings'
    globals.__RUVYXA_REQUEST_PATH__ = '/settings'
    await withGlobals(globals, async () => {
      const router = getRouterInstance()
      await router.navigate('/gallery/photo', {})
      // `/gallery/photo` has no registered tree factory here, so the router
      // falls through to a document load rather than overlaying anything.
      assert.deepEqual(
        rendered.map((tree) => tree.intercept),
        [],
      )
    })
  })

  it('never overlays a route on itself', async () => {
    // Standing on the real page and following a link to it again is a refresh,
    // not an interception — the table is inherited by every route below the
    // level, including the intercepted one.
    const rendered = []
    const history = []
    const globals = galleryGlobals('/gallery/photo', rendered, history)
    globals.__RUVYXA_ROUTE_PATTERN__ = '/gallery/photo'
    globals.__RUVYXA_REQUEST_PATH__ = '/gallery/photo'
    globals.__RUVYXA_INTERCEPTS__['/gallery/photo'] = [
      { level: 'app/gallery', name: 'modal', target: '/gallery/photo' },
    ]
    globals.__RUVYXA_ROUTES__['/gallery/photo'] = (context) => ({ ...context })
    await withGlobals(globals, async () => {
      const router = getRouterInstance()
      await router.navigate('/gallery/photo', {})
      for (const tree of rendered) {
        assert.equal(tree.intercept, undefined)
      }
    })
  })
})
