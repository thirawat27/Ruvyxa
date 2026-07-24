/**
 * Generated-entry source templates.
 *
 * A page's element tree — page wrapped in its layouts, wrapped in the routing
 * context — used to be re-implemented in five places: the Rust bundler's
 * `build_entry_source`, the dev server's SSR/SSG/client bundlers in
 * `worker-pool.mjs`, the one-shot `ssr-renderer.mjs`, and the serverless
 * `adapter-runner.mjs`. Any change to composition had to land in all of them or
 * a route would render differently depending on how it was served.
 *
 * These helpers are the single JavaScript source for that shape.
 * `crates/ruvyxa_bundler/src/output.rs` carries the Rust mirror, and
 * `tests/packages/ruvyxa/entry-templates.test.mjs` asserts the two stay in
 * step.
 */

/** Global that carries the shared routing React context across bundles. */
export const ROUTE_CONTEXT_GLOBAL = '__RUVYXA_ROUTE_CONTEXT__'

/** Global registry of route pattern to tree factory, read by the client router. */
export const ROUTE_REGISTRY_GLOBAL = '__RUVYXA_ROUTES__'

/** Local name the emitted prelude binds the shared routing context to. */
export const ROUTE_CONTEXT_LOCAL = '__ruvyxaRouteContext'

/** Local name the emitted prelude binds the error/not-found boundary class to. */
export const ROUTE_BOUNDARY_LOCAL = '__ruvyxaBoundary'

/**
 * Emit the inline error / not-found boundary class.
 *
 * A generated entry cannot import `@ruvyxa/react` (an app may render plain
 * React pages and never install it), so the boundary is defined inline. It
 * distinguishes two failures by the own property `notFound()` stamps on its
 * error — see `NOT_FOUND_PROPERTY` in `@ruvyxa/react`:
 *
 * - `error.__ruvyxaNotFound` → render `not-found.tsx` when one is present, or
 *   rethrow so an ancestor boundary can handle it.
 * - any other error → render `error.tsx` with `{ error, reset }`, or rethrow.
 *
 * Emit this exactly once per generated module, next to
 * {@link routeContextPrelude}; a second `class` declaration would not parse.
 */
export function routeBoundaryPrelude() {
  return `class ${ROUTE_BOUNDARY_LOCAL} extends React.Component {
  constructor(props) {
    super(props)
    this.state = { error: null }
    this.reset = () => this.setState({ error: null })
  }
  static getDerivedStateFromError(error) {
    return { error }
  }
  render() {
    const error = this.state.error
    if (error) {
      if (error && error.__ruvyxaNotFound) {
        if (this.props.notFound) return React.createElement(this.props.notFound, null)
        throw error
      }
      if (this.props.errorFallback) {
        return React.createElement(this.props.errorFallback, { error, reset: this.reset })
      }
      throw error
    }
    return this.props.children
  }
}`
}

/**
 * Emit the shared routing context binding.
 *
 * Created on `globalThis` instead of imported so a generated entry never has to
 * depend on `@ruvyxa/react`; an app may render plain React pages and not
 * install it at all. Both the provider here and the package's hooks reach the
 * same context object regardless of which loads first.
 *
 * Emit this exactly once per generated module — `adapter-runner.mjs` puts many
 * route definitions in one file, and a second `const` would be a redeclaration.
 */
export function routeContextPrelude() {
  return `const ${ROUTE_CONTEXT_LOCAL} = (globalThis.${ROUTE_CONTEXT_GLOBAL} ||= React.createContext(null))`
}

/**
 * Emit a function that builds a route's element tree from a render context.
 *
 * The composition, innermost to outermost: the page, wrapped in the error /
 * not-found boundary when either special is present, wrapped in a
 * `<Suspense fallback={<Loading/>}>` when a `loading.tsx` is present, wrapped in
 * the segment layouts (root-to-leaf), wrapped in the routing context provider.
 * Both sit inside the layouts so a layout stays visible while its page is
 * loading or has failed — matching Next.js.
 *
 * The boundary must sit *inside* the Suspense, not outside. A synchronous throw
 * — an ordinary error or `notFound()` — that reaches a Suspense boundary during
 * a streaming server render makes React emit the Suspense fallback and defer the
 * error boundary to the client, so the page would flash `loading.tsx` on the
 * server instead of its error/not-found UI. With the boundary nested inside, it
 * catches the throw first and renders the right UI on the server; a thrown
 * promise (real async loading) passes through it to the Suspense as usual.
 *
 * When `errorName` or `notFoundName` is supplied the module must also emit
 * {@link routeBoundaryPrelude} so `${ROUTE_BOUNDARY_LOCAL}` is in scope.
 *
 * @param {object} options
 * @param {string} options.name Function name to declare.
 * @param {string} options.pageName Identifier the page component is bound to.
 * @param {string[]} options.layoutNames Layout identifiers, root-to-leaf.
 * @param {string} options.routePath Route pattern, e.g. `/blog/[slug]`.
 * @param {string|null} [options.errorName] `error.tsx` component identifier.
 * @param {string|null} [options.loadingName] `loading.tsx` component identifier.
 * @param {string|null} [options.notFoundName] `not-found.tsx` component identifier.
 */
export function routeTreeFunction({
  name,
  pageName,
  layoutNames,
  routePath,
  errorName = null,
  loadingName = null,
  notFoundName = null,
}) {
  const lines = [
    `  let tree = React.createElement(${pageName}, { params: ctx.params ?? {}, requestPath: ctx.path })`,
  ]
  // Boundary first (inner) so a synchronous throw is caught before it reaches
  // the Suspense; Suspense second (outer) so async loading still shows.
  if (errorName || notFoundName) {
    lines.push(
      `  tree = React.createElement(${ROUTE_BOUNDARY_LOCAL}, { errorFallback: ${errorName ?? 'null'}, notFound: ${notFoundName ?? 'null'} }, tree)`,
    )
  }
  if (loadingName) {
    lines.push(
      `  tree = React.createElement(React.Suspense, { fallback: React.createElement(${loadingName}, null) }, tree)`,
    )
  }
  lines.push(`  for (const Layout of [${layoutNames.join(', ')}].reverse()) {
    tree = React.createElement(Layout, null, tree)
  }`)
  lines.push(`  return React.createElement(${ROUTE_CONTEXT_LOCAL}.Provider, {
    value: { pathname: ctx.path, params: ctx.params ?? {}, route: ${JSON.stringify(routePath)} },
  }, tree)`)
  return `function ${name}(ctx) {\n${lines.join('\n')}\n}`
}

/**
 * Whether a route's specials require the inline boundary class in scope.
 *
 * `loading.tsx` alone needs only `React.Suspense`, which is always available.
 */
export function needsRouteBoundary({ errorName = null, notFoundName = null } = {}) {
  return Boolean(errorName || notFoundName)
}

/**
 * Emit `__ruvyxaRecovery(ctx, error)`: the not-found tree, or `null`.
 *
 * `renderToPipeableStream` does not run error boundaries on the server — a throw
 * inside a Suspense boundary streams the fallback and recovers on the client. To
 * render `not-found.tsx` on the *server* (so a 404 works without JavaScript), the
 * SSR entry captures the thrown error and, when it is a `notFound()` signal,
 * re-renders this tree: the not-found component in place of the page, still
 * inside the layouts and routing context.
 *
 * Deliberately scoped to `notFound()` and nothing else. A general error also
 * reaches `onError`, but only after passing any error boundary in the user's own
 * page — recovering on every `onError` would override a page that already
 * handled its error. `error.tsx` therefore recovers on the client (as it does in
 * Next.js), while `not-found.tsx`, which no page would intercept, recovers on the
 * server.
 */
export function routeRecoveryFunction({ layoutNames, routePath, notFoundName }) {
  if (!notFoundName) return ''
  return `function __ruvyxaRecovery(ctx, error) {
  if (!(error && error.__ruvyxaNotFound)) return null
  let tree = React.createElement(${notFoundName}, null)
  for (const Layout of [${layoutNames.join(', ')}].reverse()) {
    tree = React.createElement(Layout, null, tree)
  }
  return React.createElement(${ROUTE_CONTEXT_LOCAL}.Provider, {
    value: { pathname: ctx.path, params: ctx.params ?? {}, route: ${JSON.stringify(routePath)} },
  }, tree)
}`
}

/**
 * Emit the registration that lets the client router re-render a visited route.
 *
 * `import()` caches by URL, so a bundle that has already executed will not run
 * again on a return visit. The router re-renders from this registry instead.
 */
export function routeRegistration({ name, routePath }) {
  return `;(globalThis.${ROUTE_REGISTRY_GLOBAL} ||= {})[${JSON.stringify(routePath)}] = ${name}`
}

/**
 * Build the browser hydration entry for one route.
 *
 * @param {object} options
 * @param {string[]} options.imports Import statements for page, layouts, and specials.
 * @param {string} options.pageName Identifier the page component is bound to.
 * @param {string[]} options.layoutNames Layout identifiers, root-to-leaf.
 * @param {string} options.routePath Route pattern for the registry key.
 * @param {string} options.requestPathLiteral JS literal for the fallback path.
 * @param {string} options.paramsLiteral JS literal for the fallback params.
 * @param {string|null} [options.errorName] `error.tsx` component identifier.
 * @param {string|null} [options.loadingName] `loading.tsx` component identifier.
 * @param {string|null} [options.notFoundName] `not-found.tsx` component identifier.
 */
export function clientEntrySource({
  imports,
  pageName,
  layoutNames,
  routePath,
  requestPathLiteral,
  paramsLiteral,
  errorName = null,
  loadingName = null,
  notFoundName = null,
}) {
  const boundary = needsRouteBoundary({ errorName, notFoundName })
    ? `\n${routeBoundaryPrelude()}\n`
    : ''
  return `import React from "react"
import { hydrateRoot } from "react-dom/client"
${imports.join('\n')}

${routeContextPrelude()}
${boundary}
${routeTreeFunction({ name: '__ruvyxaTree', pageName, layoutNames, routePath, errorName, loadingName, notFoundName })}
${routeRegistration({ name: '__ruvyxaTree', routePath })}

const __ruvyxaCtx = {
  path: globalThis.__RUVYXA_REQUEST_PATH__ ?? ${requestPathLiteral},
  params: globalThis.__RUVYXA_ROUTE_PARAMS__ ?? ${paramsLiteral},
}
const __ruvyxaTreeElement = __ruvyxaTree(__ruvyxaCtx)

if (globalThis.__RUVYXA_ROOT__) {
  globalThis.__RUVYXA_ROOT__.render(__ruvyxaTreeElement)
} else {
  globalThis.__RUVYXA_ROOT__ = hydrateRoot(document, __ruvyxaTreeElement)
}
window.__RUVYXA_HYDRATED = true
`
}

/**
 * Build a Node SSR entry that streams through `renderToPipeableStream`.
 *
 * @param {object} options
 * @param {string[]} options.imports Import statements for page, layouts, and specials.
 * @param {string} options.pageName Identifier the page component is bound to.
 * @param {string[]} options.layoutNames Layout identifiers, root-to-leaf.
 * @param {string} options.routePath Route pattern for the routing context.
 * @param {'onAllReady'|'onShellReady'} [options.readyEvent] Stream checkpoint.
 *   `onShellReady` is what makes a partial prerender emit its static shell
 *   before dynamic slots resolve.
 * @param {boolean} [options.tolerateStreamErrors] Keep streaming when a slot
 *   throws, instead of rejecting the whole render.
 * @param {string|null} [options.errorName] `error.tsx` component identifier.
 * @param {string|null} [options.loadingName] `loading.tsx` component identifier.
 * @param {string|null} [options.notFoundName] `not-found.tsx` component identifier.
 */
export function nodeSsrEntrySource({
  imports,
  pageName,
  layoutNames,
  routePath,
  readyEvent = 'onAllReady',
  tolerateStreamErrors = false,
  errorName = null,
  loadingName = null,
  notFoundName = null,
}) {
  const boundary = needsRouteBoundary({ errorName, notFoundName })
    ? `\n${routeBoundaryPrelude()}\n`
    : ''

  // Only `not-found.tsx` recovers on the server (see routeRecoveryFunction).
  const serverRecovers = Boolean(notFoundName)
  const recovery = serverRecovers
    ? `\n${routeRecoveryFunction({ layoutNames, routePath, notFoundName })}\n`
    : ''

  return `import React from "react"
import * as ReactDomServer from "react-dom/server"
import { Writable } from "node:stream"
${imports.join('\n')}

${routeContextPrelude()}
${boundary}
${routeTreeFunction({ name: '__ruvyxaTree', pageName, layoutNames, routePath, errorName, loadingName, notFoundName })}
${recovery}
export async function render(ctx) {
  const tree = __ruvyxaTree(ctx)

  if (typeof ReactDomServer.renderToPipeableStream !== "function") {
${
  serverRecovers
    ? `    try {
      return "<!doctype html>" + ReactDomServer.renderToString(tree)
    } catch (error) {
      const recovery = __ruvyxaRecovery(ctx, error)
      if (recovery) return "<!doctype html>" + ReactDomServer.renderToString(recovery)
      throw error
    }`
    : '    return "<!doctype html>" + ReactDomServer.renderToString(tree)'
}
  }

  return new Promise((resolve, reject) => {
    const chunks = []
    let captured = null
    const writable = new Writable({
      write(chunk, encoding, callback) {
        chunks.push(chunk)
        callback()
      },
    })

    const { pipe } = ReactDomServer.renderToPipeableStream(tree, {
      ${readyEvent}() {${
        serverRecovers
          ? `
        // A deferred not-found still fired onError. Send the server-rendered
        // not-found UI instead of the streamed loading fallback.
        if (captured) {
          const recovery = __ruvyxaRecovery(ctx, captured)
          if (recovery) {
            resolve("<!doctype html>" + ReactDomServer.renderToString(recovery))
            return
          }
        }`
          : ''
      }
        pipe(writable)
        writable.on("finish", () => {
          const html = Buffer.concat(chunks).toString("utf8")
          resolve(html.trimStart().toLowerCase().startsWith("<!doctype") ? html : "<!doctype html>" + html)
        })
      },
      onShellError(error) {${
        serverRecovers
          ? `
        const recovery = __ruvyxaRecovery(ctx, error)
        if (recovery) {
          resolve("<!doctype html>" + ReactDomServer.renderToString(recovery))
          return
        }`
          : ''
      }
        reject(error)
      },
      onError(error) {
        ${serverRecovers ? 'if (!captured) captured = error\n        ' : ''}${
          tolerateStreamErrors || serverRecovers
            ? 'if (globalThis.process?.env?.RUVYXA_DEBUG) console.error("[ruvyxa] streaming render error", error)'
            : 'reject(error)'
        }
      },
    })
  })
}
`
}
