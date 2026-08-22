/**
 * Turning a route's server-components graph into a document.
 *
 * A React Server Components render is two passes over two module graphs that
 * never share a React instance:
 *
 *   1. The **server graph**, compiled with the `react-server` export condition,
 *      runs the page and its layouts and emits a *Flight payload* — a
 *      serialised element tree in which every `'use client'` component appears
 *      as a reference id rather than as code.
 *   2. The **SSR pass**, running here with the ordinary React, reads that
 *      payload, resolves each reference to the real browser module, and renders
 *      the whole thing to HTML so the first paint is a page rather than an empty
 *      shell.
 *
 * The same payload is then inlined into the document, and the browser repeats
 * step 2 against the same references. That is what makes hydration match: both
 * realms decode one payload with one registry, so they cannot disagree about
 * which module an id names.
 *
 * **Why the two graphs can share a process.** Ruvyxa compiles the server graph
 * itself, with `react-server` in the resolver's condition list, so React's
 * server build is linked *into* that bundle. The React this module imports is a
 * different object in the same process, which is exactly the separation RSC
 * requires. The alternative — a worker thread started with
 * `--conditions=react-server` — was measured to work too, and was rejected
 * because it cannot run on the worker runtimes the adapters target, and because
 * bundling React is something this codebase's linker already does for every
 * client bundle it emits.
 *
 * **What lives here and what does not.** This module renders; it does not
 * compile or cache. The bundles it is handed are built and keyed by the host —
 * `worker-pool.mjs` for `ruvyxa dev`, `ruvyxa start`, and the build's
 * prerender pass — because bundle caching and invalidation are already that
 * host's job and a second cache would be a second answer about staleness.
 */

import React from 'react'
import { renderToReadableStream as renderHtmlStream } from 'react-dom/server'

import { RSC_SSR_PACKAGE, clientManifest } from './client-references.mjs'
import { ROUTE_CONTEXT_GLOBAL } from './entry-templates.mjs'
import { installClientReferenceRuntime } from './rsc-client-runtime.mjs'

/**
 * Render one server-components route to HTML plus the payload that produced it.
 *
 * @param {object} options
 * @param {{ flight: (ctx: object, manifest: object, options: object) => Promise<ReadableStream>|ReadableStream }} options.serverModule
 *   The compiled `react-server` bundle for this route.
 * @param {{ id: string }[]} options.references Client modules the server graph
 *   turned into references. Their browser modules must already be registered —
 *   importing the SSR registry bundle is what does that, and the caller does it
 *   because the caller owns the module cache.
 * @param {{ path: string, params: object }} options.ctx Render context.
 * @param {string} options.routePath Route pattern, for the routing context.
 * @returns {Promise<{ html: string, payload: string }>}
 */
export async function renderServerComponents({ serverModule, references, ctx, routePath }) {
  if (typeof serverModule?.flight !== 'function') {
    throw new Error(
      'RUV1862 the server-components bundle exports no flight(); the route was compiled without the react-server entry',
    )
  }

  // A server component that throws reaches `onError` rather than rejecting the
  // stream, so the first error is captured and rethrown once both consumers
  // have finished. Rejecting from inside `onError` would leave the tee's other
  // branch unread and the render hanging.
  let failure = null
  const stream = await serverModule.flight(ctx, flightManifest(references), {
    onError(error) {
      failure ??= error
    },
  })

  // One render, two readers: the payload the browser will replay, and the HTML
  // it will hydrate. Rendering twice would run every server component twice and
  // could produce two different trees.
  const [forHtml, forPayload] = stream.tee()
  const [html, payload] = await Promise.all([
    flightStreamToHtml(forHtml, ctx, routePath),
    readStreamText(forPayload),
  ])
  if (failure) throw failure
  return { html, payload }
}

/**
 * Decode a Flight stream and render it to a full HTML document.
 *
 * The routing context provider is added here rather than in the server graph:
 * `React.createContext` does not exist in the `react-server` build, because a
 * context read is a client concern. The browser entry wraps the decoded element
 * in the same provider with the same value, so the markup matches.
 */
export async function flightStreamToHtml(stream, ctx, routePath) {
  // Installed here rather than when this module loads. `__webpack_require__` is
  // how a library decides it is running inside webpack — `sass` reads it and
  // then reaches for `__non_webpack_require__` — so the claim is made only once
  // a render actually needs it, and never in a process that merely imported the
  // server-components pipeline.
  installClientReferenceRuntime()
  // Dynamic, and by a constant rather than a literal, because the package is an
  // optional peer: a project that never writes `serverComponents` must not fail
  // to load this module. It is also why `knip.json` has to be told the
  // dependency is used — nothing static names it anywhere in the tree.
  const { createFromReadableStream } = await import(RSC_SSR_PACKAGE)
  const element = await createFromReadableStream(stream, {
    serverConsumerManifest: {
      moduleMap: ssrModuleMap(),
      // Every client module a server-components route reaches is linked into
      // that route's one browser bundle, so a reference names no chunk to fetch
      // and the prefix is never joined to anything.
      moduleLoading: { prefix: '/' },
    },
  })

  const RouteContext = (globalThis[ROUTE_CONTEXT_GLOBAL] ??= React.createContext(null))
  const tree = React.createElement(
    RouteContext.Provider,
    { value: routeContextValue(ctx, routePath) },
    element,
  )

  const htmlStream = await renderHtmlStream(tree)
  await htmlStream.allReady
  const html = await readStreamText(htmlStream)
  return html.trimStart().toLowerCase().startsWith('<!doctype') ? html : `<!doctype html>${html}`
}

/** The context value both the SSR pass and the browser entry provide. */
export function routeContextValue(ctx, routePath) {
  return {
    pathname: ctx.path,
    params: ctx.params ?? {},
    route: routePath,
    flight: undefined,
  }
}

/**
 * The manifest React serialises a client reference against.
 *
 * `chunks: []` for every reference, because a server-components route ships one
 * browser bundle containing all of them. An empty list is what tells React to
 * skip `__webpack_chunk_load__` and go straight to the registry — and it is why
 * this feature needs no chunk URLs, no chunk manifest file, and no second
 * static route to serve them from.
 */
function flightManifest(references) {
  return clientManifest(references.map((reference) => ({ id: reference.id, chunks: [] })))
}

/**
 * The map the SSR decoder resolves ids through.
 *
 * `client.edge` looks up `moduleMap[id][exportName]` for names no build can
 * enumerate ahead of time, so both levels answer anything — the same reason
 * `clientManifest` is a Proxy. The metadata it returns is then handed to
 * `__webpack_require__`, which is where the real module comes from.
 */
function ssrModuleMap() {
  return new Proxy(Object.create(null), {
    get(_target, id) {
      if (typeof id !== 'string') return undefined
      return new Proxy(Object.create(null), {
        get: (_inner, name) =>
          typeof name === 'string' ? { id, chunks: [], name, async: false } : undefined,
        has: () => true,
      })
    },
    has: () => true,
  })
}

/** Read a whole `ReadableStream` of bytes as UTF-8 text. */
export async function readStreamText(stream) {
  const decoder = new TextDecoder()
  const reader = stream.getReader()
  let text = ''
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    text += decoder.decode(value, { stream: true })
  }
  return text + decoder.decode()
}
