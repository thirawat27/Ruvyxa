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

import { RSC_SSR_PACKAGE, clientManifest, serverManifest } from './client-references.mjs'
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
 * @param {boolean} [options.html] Also render the payload to HTML. A soft
 *   navigation asks for the payload alone: the browser already has a document,
 *   and rendering markup it will throw away would run the SSR pass for nothing.
 * @param {unknown} [options.formState] What a `useActionState` form posted
 *   without JavaScript returned, for the HTML renderer to replay.
 * @returns {Promise<{ html: string|null, payload: string }>}
 */
export async function renderServerComponents({
  serverModule,
  references,
  ctx,
  routePath,
  html: withHtml = true,
  formState = null,
}) {
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
  //
  // A payload-only render reads the stream once. `tee()` on a stream whose
  // second branch is never read buffers every chunk waiting for it, so the
  // branch is not created rather than created and dropped.
  if (!withHtml) {
    const payload = await readStreamText(stream)
    if (failure) throw failure
    return { html: null, payload }
  }
  const [forHtml, forPayload] = stream.tee()
  const [html, payload] = await Promise.all([
    flightStreamToHtml(forHtml, ctx, routePath, { formState }),
    readStreamText(forPayload),
  ])
  if (failure) throw failure
  return { html, payload }
}

/**
 * Render one server-components route as a stream, plus the payload behind it.
 *
 * The streaming half of {@link renderServerComponents}, for a route whose
 * document is produced per request and cached by nobody. Nothing waits for the
 * whole tree: React emits the shell as soon as it is ready and each `Suspense`
 * boundary as it resolves, so a slow server component delays the part of the
 * page that is waiting on it and nothing else.
 *
 * The payload is returned as a *promise* rather than a value because it is only
 * complete when the Flight render is, which is after the caller has already
 * started sending bytes. The host awaits it at the end of the stream and writes
 * the data block then — the browser needs it before hydration, not before the
 * first paint, and hydration cannot begin until the document has been parsed.
 *
 * @returns {Promise<{ stream: ReadableStream, payload: Promise<string> }>}
 */
export async function renderServerComponentsStream({
  serverModule,
  references,
  ctx,
  routePath,
  formState = null,
}) {
  if (typeof serverModule?.flight !== 'function') {
    throw new Error(
      'RUV1862 the server-components bundle exports no flight(); the route was compiled without the react-server entry',
    )
  }
  // A server component that throws after the shell has been sent cannot change
  // the response: the status line is already gone. React writes what it can and
  // reports here, which is the only place left that can log it.
  const failures = []
  const flight = await serverModule.flight(ctx, flightManifest(references), {
    onError(error) {
      failures.push(error)
    },
  })
  const [forHtml, forPayload] = flight.tee()
  // Started now, awaited by the caller at the end. Not awaited here: the point
  // of this function is to return before the render has finished.
  const payload = readStreamText(forPayload)
  const stream = await flightStreamToHtmlStream(forHtml, ctx, routePath, { formState })
  return { stream, payload, failures }
}

/**
 * Decode a Flight stream and render it to a full HTML document.
 *
 * The routing context provider is added here rather than in the server graph:
 * `React.createContext` does not exist in the `react-server` build, because a
 * context read is a client concern. The browser entry wraps the decoded element
 * in the same provider with the same value, so the markup matches.
 */
export async function flightStreamToHtml(stream, ctx, routePath, { formState = null } = {}) {
  const html = await flightStreamToHtmlStream(stream, ctx, routePath, { complete: true, formState })
  return await readStreamText(html)
}

/**
 * The same render, handed back as a stream instead of a string.
 *
 * The difference is one `await`. `renderToReadableStream` resolves as soon as
 * the shell is ready and keeps emitting as each `Suspense` boundary resolves;
 * waiting for `allReady` throws that away and makes the slowest server
 * component decide when the *first* byte leaves. A caller that can pass bytes
 * through as they arrive should not wait, and a caller that has to produce one
 * string — a prerender writing a file, an ISR entry going into a cache — has no
 * choice, which is what `complete` selects.
 *
 * @param {object} [options]
 * @param {boolean} [options.complete] Wait for every boundary before returning.
 * @param {unknown} [options.formState] What the `useActionState` form on this
 *   page returned when it was posted without JavaScript. React writes a marker
 *   comment beside each such hook and replays the value into the one the post
 *   named, which is how a no-JS submission shows its result. `null` — every
 *   render that is not answering a form post — leaves each hook at its initial
 *   state.
 */
export async function flightStreamToHtmlStream(
  stream,
  ctx,
  routePath,
  { complete = false, formState = null } = {},
) {
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
    // The SSR pass renders a payload; it never *acts* on one. A server function
    // in the tree is markup here — React writes the reference into the form and
    // the browser calls it — so reaching this is a bug worth naming rather than
    // a call worth making.
    callServer() {
      throw new Error('RUV1868 a server function was called during the server-side render pass')
    },
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

  const htmlStream = await renderHtmlStream(tree, { formState })
  if (complete) await htmlStream.allReady
  return withDoctype(htmlStream)
}

/**
 * The same stream, guaranteed to open with a doctype.
 *
 * React writes one itself when the tree it was handed starts at `<html>`, which
 * a route with a root layout always does — and writes none when it does not, so
 * a document that opens at `<main>` would be served in quirks mode. Only the
 * first chunk is inspected, and only far enough to answer that: prepending
 * unconditionally is what produced two of them.
 */
function withDoctype(stream) {
  const reader = stream.getReader()
  const encoder = new TextEncoder()
  let first = true
  return new ReadableStream({
    async pull(controller) {
      const { done, value } = await reader.read()
      if (done) {
        controller.close()
        return
      }
      if (first) {
        first = false
        const opening = new TextDecoder().decode(value.slice(0, 15)).trimStart().toLowerCase()
        if (!opening.startsWith('<!doctype')) {
          controller.enqueue(encoder.encode('<!doctype html>'))
        }
      }
      controller.enqueue(value)
    },
    cancel(reason) {
      return reader.cancel(reason)
    },
  })
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
export function flightManifest(references) {
  return clientManifest(references.map((reference) => ({ id: reference.id, chunks: [] })))
}

/**
 * Run one of a route's server functions and return the payload it produced.
 *
 * The deployed twin of `worker_pool.rs`'s `render_rsc_action`. It exists here
 * rather than in the generated route module because both hosts have to hand
 * `callServerFunction` the *same* two manifests: a reply encoded against a
 * different client manifest names references the caller cannot resolve, and the
 * server manifest is what lets an argument carry an action of its own —
 * `remove.bind(null, id)` passed to `<form action={…}>` is the ordinary way
 * that happens.
 *
 * A deployed build could not do this at all until now. `/__ruvyxa/rsc` answered
 * `GET` and refused `POST` with a `405`, so clicking anything wired to a server
 * function on a deployed server-components page threw `Connection closed.` in
 * the browser and left a blank document — while the same page worked under
 * `ruvyxa dev` and `ruvyxa start`.
 *
 * @param {object} options
 * @param {{ callServerFunction: Function }} options.actionModule The compiled
 *   `react-server` action bundle for this route.
 * @param {{ id: string }[]} options.references The route's client references.
 * @param {string} options.reference The `'use server'` id the caller named.
 * @param {string|FormData} options.body What React's `encodeReply` produced.
 * @returns {Promise<string>} The Flight payload the function's return value
 *   encodes to, which may itself contain client references.
 */
/**
 * Run the action a plain form post named, for a browser with no JavaScript.
 *
 * The deployed twin of `posted_form()` plus `runFormAction` on the native host.
 * `null` when the post named no action — the ordinary answer for any other form
 * on the page, and the caller then renders the route as it would have without a
 * body.
 *
 * The `formState` half is what makes `useActionState` show its answer without
 * JavaScript: React writes an extra `$ACTION_KEY` field for a form whose action
 * came from that hook, and the value returned here is the token the HTML
 * renderer replays it from. Without it the action runs and the page renders its
 * initial state, which is what a deployed build did — the submission reached a
 * `200` carrying no evidence it had happened.
 */
export async function runRouteFormAction({ actionModule, formData }) {
  if (typeof actionModule?.runFormAction !== 'function') return null
  return await actionModule.runFormAction({ formData, serverManifest: serverManifest() })
}

export async function callRouteServerFunction({ actionModule, references, reference, body }) {
  if (typeof actionModule?.callServerFunction !== 'function') {
    throw new Error(
      'RUV1866 this route was compiled without a server-function bundle, so nothing can be called on it',
    )
  }
  // Same shape as the render path: a function that throws reaches `onError`
  // rather than rejecting the stream, so the first error is captured and
  // rethrown once the payload has been read.
  let failure = null
  const stream = await actionModule.callServerFunction({
    reference,
    body,
    manifest: flightManifest(references),
    serverManifest: serverManifest(),
    options: {
      onError(error) {
        failure ??= error
      },
    },
  })
  const payload = await readStreamText(stream)
  if (failure) throw failure
  return payload
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
