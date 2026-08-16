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

/**
 * Global carrying the route pattern this document was served from.
 *
 * The registry above is keyed by pattern (`/blog/[slug]`), not by URL, so the
 * client router needs the pattern to look up its own initial route.
 */
export const ROUTE_PATTERN_GLOBAL = '__RUVYXA_ROUTE_PATTERN__'

/** Local name the emitted prelude binds the shared routing context to. */
export const ROUTE_CONTEXT_LOCAL = '__ruvyxaRouteContext'

/** Local name the emitted prelude binds the error/not-found boundary class to. */
export const ROUTE_BOUNDARY_LOCAL = '__ruvyxaBoundary'

/** Local name bound to the route-metadata merge helper. */
export const META_RESOLVE_LOCAL = '__ruvyxaResolveMeta'

/** Local name bound to the helper that turns merged metadata into elements. */
export const META_ELEMENT_LOCAL = '__ruvyxaMetaElement'

/** Local name bound to the helper that rewrites `<html lang>` on a rendered document. */
export const META_LANG_LOCAL = '__ruvyxaApplyLang'

/** Identifier prefix for the namespace imports metadata is read from. */
export const META_SOURCE_PREFIX = '__ruvyxaMeta'

/**
 * Build the namespace imports a route's metadata is merged from.
 *
 * A page and its layouts are already imported for their default export, but a
 * default import cannot see a sibling `export const meta`. Re-importing the same
 * specifier as a namespace is the smallest change that exposes it: ESM gives
 * both statements the same module instance, and every bundler in the pipeline
 * collapses them to one record.
 *
 * `importPaths` must be ordered root layout → leaf layout → page, which is the
 * order {@link routeMetaPrelude}'s resolver treats as least → most specific.
 *
 * Mirrored by `meta_source_imports()` in `crates/ruvyxa_bundler/src/output.rs`.
 *
 * @param {string[]} importPaths Module specifiers, already normalized.
 * @param {string} [namePrefix] Identifier prefix, unique per route in a file
 *   that defines several routes (`adapter-runner.mjs`).
 */
export function metaSourceImports(importPaths, namePrefix = META_SOURCE_PREFIX) {
  const imports = []
  const metaNames = []
  importPaths.forEach((importPath, index) => {
    const name = `${namePrefix}${index}`
    imports.push(`import * as ${name} from ${JSON.stringify(importPath)}`)
    metaNames.push(name)
  })
  return { imports, metaNames }
}

/**
 * Emit the route-metadata helpers: merge, element construction, and the
 * `<html lang>` rewrite.
 *
 * Metadata is merged least-specific first (root layout → page), so a page
 * overrides its layouts field by field. `titleTemplate` applies only to a title
 * declared *below* the level that set the template, matching Next.js: a layout's
 * template formats its pages' titles but not its own.
 *
 * The elements are ordinary `<title>`/`<meta>`/`<link>` nodes. React 19 hoists
 * those into `<head>` from anywhere in the tree, so this needs no cooperation
 * from the Rust document composer and works identically for SSR, SSG, PPR, and
 * hydration.
 *
 * Resolution is synchronous by design: a `meta` function runs during render, and
 * an async one would resolve after the shell has already been flushed with no
 * title. `meta` must therefore be an object or a synchronous function.
 *
 * Emit this exactly once per generated module, next to
 * {@link routeContextPrelude}.
 *
 * @param {object} [options]
 * @param {boolean} [options.lang] Include the `<html lang>` rewrite. Only a
 *   server entry has a document string to rewrite; shipping it to the browser
 *   would be dead bytes on every route bundle.
 */
export function routeMetaPrelude({ lang = true } = {}) {
  const langHelper = lang
    ? `

function ${META_LANG_LOCAL}(html, lang) {
  if (typeof html !== "string" || typeof lang !== "string" || lang === "") return html
  const match = /<html\\b[^>]*>/i.exec(html)
  if (!match) return html
  const value = lang.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;")
  const attribute = /\\slang\\s*=\\s*("[^"]*"|'[^']*'|[^\\s>]+)/i
  const tag = attribute.test(match[0])
    ? match[0].replace(attribute, () => ' lang="' + value + '"')
    : match[0].replace(/^<html/i, () => '<html lang="' + value + '"')
  return html.slice(0, match.index) + tag + html.slice(match.index + match[0].length)
}`
    : ''

  return `function ${META_RESOLVE_LOCAL}(sources, ctx) {
  const merged = {}
  let template = null
  let templateDepth = -1
  let titleDepth = -1
  for (let depth = 0; depth < sources.length; depth += 1) {
    const source = sources[depth]
    const declared = source && source.meta
    const resolved = typeof declared === "function" ? declared(ctx) : declared
    if (!resolved || typeof resolved !== "object") continue
    if (typeof resolved.titleTemplate === "string") {
      template = resolved.titleTemplate
      templateDepth = depth
    }
    for (const key of Object.keys(resolved)) {
      if (resolved[key] !== undefined) merged[key] = resolved[key]
    }
    if (typeof resolved.title === "string") titleDepth = depth
  }
  if (template && titleDepth > templateDepth && typeof merged.title === "string") {
    merged.title = template.replace("%s", () => merged.title)
  }
  delete merged.titleTemplate
  return merged
}

function ${META_ELEMENT_LOCAL}(meta) {
  if (!meta || typeof meta !== "object") return null
  const children = []
  const add = (type, props) => {
    children.push(React.createElement(type, Object.assign({ key: type + children.length }, props)))
  }
  const title = typeof meta.title === "string" && meta.title !== "" ? meta.title : null
  const description = typeof meta.description === "string" ? meta.description : null
  const canonical = typeof meta.canonical === "string" ? meta.canonical : null
  const image = typeof meta.image === "string" ? meta.image : null
  if (title) add("title", { children: title })
  if (description) add("meta", { name: "description", content: description })
  if (canonical) add("link", { rel: "canonical", href: canonical })
  const robots = typeof meta.robots === "string" ? meta.robots : meta.noindex ? "noindex, nofollow" : null
  if (robots) add("meta", { name: "robots", content: robots })
  for (const alternate of Array.isArray(meta.alternates) ? meta.alternates : []) {
    if (alternate && alternate.href && alternate.hreflang) {
      add("link", { rel: "alternate", hrefLang: alternate.hreflang, href: alternate.href })
    }
  }
  if (title || description || image) {
    if (title) add("meta", { property: "og:title", content: title })
    if (description) add("meta", { property: "og:description", content: description })
    add("meta", { property: "og:type", content: meta.type || "website" })
    if (canonical) add("meta", { property: "og:url", content: canonical })
    if (meta.siteName) add("meta", { property: "og:site_name", content: meta.siteName })
    if (meta.locale) add("meta", { property: "og:locale", content: meta.locale })
    if (image) add("meta", { property: "og:image", content: image })
    if (image && meta.imageAlt) add("meta", { property: "og:image:alt", content: meta.imageAlt })
    add("meta", { name: "twitter:card", content: meta.card || (image ? "summary_large_image" : "summary") })
    if (title) add("meta", { name: "twitter:title", content: title })
    if (description) add("meta", { name: "twitter:description", content: description })
    if (image) add("meta", { name: "twitter:image", content: image })
  }
  if (children.length === 0) return null
  return children
}${langHelper}`
}

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
 * @param {string|null} [options.pageModuleName] Namespace containing an optional Flight export.
 * @param {string[]} options.layoutNames Layout identifiers, root-to-leaf.
 * @param {string} options.routePath Route pattern, e.g. `/blog/[slug]`.
 * @param {string|null} [options.errorName] `error.tsx` component identifier.
 * @param {string|null} [options.loadingName] `loading.tsx` component identifier.
 * @param {string|null} [options.notFoundName] `not-found.tsx` component identifier.
 * @param {string[]} [options.metaNames] Namespace identifiers from
 *   {@link metaSourceImports}, least specific first. Omitting them emits a tree
 *   with no metadata code at all, which is what a caller that has not adopted
 *   route metadata gets.
 */
export function routeTreeFunction({
  name,
  pageName,
  pageModuleName = null,
  layoutNames,
  routePath,
  metaNames = [],
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
  // Metadata is a sibling of the layouts, not a wrapper around them: a layout
  // that suspends must not be able to hold the document title back past the
  // flushed shell. It is passed as an extra child of the provider — an element
  // array with its own keys — so no extra wrapper element is created per render.
  const metaChild =
    metaNames.length > 0
      ? `${META_ELEMENT_LOCAL}(${META_RESOLVE_LOCAL}([${metaNames.join(', ')}], ctx)), `
      : ''
  lines.push(`  return React.createElement(${ROUTE_CONTEXT_LOCAL}.Provider, {
    value: { pathname: ctx.path, params: ctx.params ?? {}, route: ${JSON.stringify(routePath)}, flight: ctx.flight },
  }, ${metaChild}tree)`)
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
    value: { pathname: ctx.path, params: ctx.params ?? {}, route: ${JSON.stringify(routePath)}, flight: ctx.flight },
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
  // `__RUVYXA_ROUTE_PATTERN__` tells the client router which registry key the
  // document was served from. Without it the router had to seed its snapshot
  // from `__RUVYXA_REQUEST_PATH__` — a concrete URL — and then failed to find
  // `__RUVYXA_ROUTES__["/blog/hello"]`, so `router.refresh()` silently rendered
  // nothing on any dynamic route until the first client navigation replaced the
  // snapshot. Emitted here so both entry generators publish it from the same
  // place they register the tree; `output.rs` mirrors this line and
  // `client_entry_matches_node_runtime_contract` keeps the two in lockstep.
  return `;(globalThis.${ROUTE_REGISTRY_GLOBAL} ||= {})[${JSON.stringify(routePath)}] = ${name}
globalThis.${ROUTE_PATTERN_GLOBAL} = ${JSON.stringify(routePath)}`
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
 * @param {string[]} [options.metaNames] Namespace identifiers from {@link metaSourceImports}.
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
  metaNames = [],
}) {
  const boundary = needsRouteBoundary({ errorName, notFoundName })
    ? `\n${routeBoundaryPrelude()}\n`
    : ''
  const meta = metaNames.length > 0 ? `\n${routeMetaPrelude({ lang: false })}\n` : ''
  return `import React from "react"
import { hydrateRoot } from "react-dom/client"
${imports.join('\n')}

${routeContextPrelude()}
${boundary}${meta}
${routeTreeFunction({ name: '__ruvyxaTree', pageName, layoutNames, routePath, errorName, loadingName, notFoundName, metaNames })}
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
  pageModuleName = null,
  layoutNames,
  routePath,
  readyEvent = 'onAllReady',
  tolerateStreamErrors = false,
  errorName = null,
  loadingName = null,
  notFoundName = null,
  metaNames = [],
}) {
  const boundary = needsRouteBoundary({ errorName, notFoundName })
    ? `\n${routeBoundaryPrelude()}\n`
    : ''
  const metaPrelude = metaNames.length > 0 ? `\n${routeMetaPrelude()}\n` : ''
  // `lang` is the one metadata field React cannot place: it belongs to the
  // `<html>` element the app itself renders, which no hoisted child can reach.
  // The finished document string is rewritten here instead, so every server
  // path — SSR, SSG, PPR, prerender, serverless — agrees.
  const applyLang =
    metaNames.length > 0
      ? `${META_LANG_LOCAL}(html, ${META_RESOLVE_LOCAL}([${metaNames.join(', ')}], ctx).lang)`
      : 'html'

  // Only `not-found.tsx` recovers on the server (see routeRecoveryFunction).
  const serverRecovers = Boolean(notFoundName)
  const recovery = serverRecovers
    ? `\n${routeRecoveryFunction({ layoutNames, routePath, notFoundName })}\n`
    : ''
  const flight = pageModuleName
    ? `\nexport async function flight(ctx) {\n  if (typeof ${pageModuleName}.flight !== "function") throw new Error("RUV1830 route does not export flight(context)")\n  return ${pageModuleName}.flight(ctx)\n}\n`
    : ''

  return `import React from "react"
import * as ReactDomServer from "react-dom/server"
import { Writable } from "node:stream"
${imports.join('\n')}

${routeContextPrelude()}
${boundary}${metaPrelude}
${routeTreeFunction({ name: '__ruvyxaTree', pageName, layoutNames, routePath, errorName, loadingName, notFoundName, metaNames })}
${recovery}
export async function render(ctx) {
  const html = await __ruvyxaRenderDocument(ctx)
  return ${applyLang}
}
${flight}

async function __ruvyxaRenderDocument(ctx) {
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
