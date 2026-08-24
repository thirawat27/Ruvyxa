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
 *
 * The two server-components entries at the bottom of this file have no Rust
 * mirror yet, because the Rust build path has no `react-server` target to emit
 * them from. That is a gap in coverage, not a difference in behaviour: nothing
 * in Rust produces a competing version of them today.
 */

import {
  RSC_BROWSER_PACKAGE,
  RSC_CLIENT_RUNTIME_SPECIFIER,
  RSC_SERVER_PACKAGE,
  clientReferenceInstallPath,
  clientReferenceRuntimePath,
  clientRegistrySource,
} from './client-references.mjs'
import { toImportPath } from './paths.mjs'
import { compareCodeUnits } from './order.mjs'
// One owner for the element id the Flight payload rides in. `rsc-client-runtime`
// is the reader and declares no import-time side effect, so importing it here
// costs nothing — unlike `rsc-client-install.mjs`, which exists to hold one.
import { RSC_PAYLOAD_ELEMENT_ID } from './rsc-client-runtime.mjs'

/** Global that carries the shared routing React context across bundles. */
export const ROUTE_CONTEXT_GLOBAL = '__RUVYXA_ROUTE_CONTEXT__'

/**
 * Element carrying route bootstrap data from the document to the bundle.
 *
 * Held with the JSON key names to
 * `tests/fixtures/client-bootstrap-conformance.json`, which the three Rust
 * writers of this element also replay.
 */
export const BOOTSTRAP_ELEMENT_ID = '__ruvyxa-bootstrap'

/**
 * Read the bootstrap data block and publish it on `globalThis`.
 *
 * The document used to carry these assignments as an executable inline
 * `<script>`. Every page had one, so any `Content-Security-Policy` without
 * `'unsafe-inline'` blocked it and hydration never started — and since the
 * parameters differ per request, a CSP hash could not cover it either.
 *
 * `type="application/json"` is a data block rather than executable script, so
 * `script-src` does not apply to it. Publishing the same globals here is what
 * keeps every reader downstream — `router.ts` above all — unchanged.
 *
 * `??=` rather than `=`: a soft navigation has already written the params for
 * the route it is entering, and this bundle may only be evaluated afterwards.
 * Overwriting would replace them with the ones the document was served with.
 *
 * Mirrored by `client_bootstrap_prelude` in `crates/ruvyxa_bundler/src/output.rs`.
 */
export function clientBootstrapPrelude() {
  return `const __ruvyxaBootstrap = (() => {
  if (typeof document === "undefined") return {}
  const el = document.getElementById(${JSON.stringify(BOOTSTRAP_ELEMENT_ID)})
  if (!el) return {}
  try {
    return JSON.parse(el.textContent || "{}")
  } catch {
    return {}
  }
})()
globalThis.__RUVYXA_ROUTE_PARAMS__ ??= __ruvyxaBootstrap.params
globalThis.__RUVYXA_REQUEST_PATH__ ??= __ruvyxaBootstrap.path
if (__ruvyxaBootstrap.csr === true) globalThis.__RUVYXA_CSR__ = true`
}

/**
 * Emit the document writers a deployed function needs, as source text.
 *
 * Every other writer of these blocks is Rust: `client_hydration_script` for a
 * live render, `inject_prerender_client_assets` for a page baked at build time,
 * and the CSR shell. A deployed build has none of them — the generated route
 * registry *is* the renderer — and it wrote no bootstrap block, no module
 * preloads, and no `<script type="module">` at all. So an SSR route in any
 * deployed function served markup that could never hydrate, and an ISR route
 * lost its script the first time it revalidated, because the revalidation
 * persists what this renderer produced over the file the build had injected.
 *
 * Returned as source rather than imported, for the reason `flightCachePrelude`
 * is: this text is emitted into a function artifact that resolves no bare or
 * sibling specifiers.
 *
 * The escaping is the JavaScript twin of `safe_json_for_script` and
 * `escape_html` in `crates/ruvyxa_dev_server/src/html_document.rs`. Held to
 * `tests/fixtures/client-bootstrap-conformance.json` — the element id, the key
 * names, and the two escaping cases — by a test that renders a real deployed
 * route module, because the values come from the request URL and a writer that
 * forgot would let a path segment close the script element.
 */
export function documentAssetsPrelude(styleHead = '') {
  return `const __RUVYXA_BOOTSTRAP_ID = ${JSON.stringify(BOOTSTRAP_ELEMENT_ID)}
const __RUVYXA_RSC_ID = ${JSON.stringify(RSC_PAYLOAD_ELEMENT_ID)}
/** The stylesheet the build emitted, linked by every document this renders. */
const __RUVYXA_STYLE_HEAD = ${JSON.stringify(styleHead)}

/** Twin of \`safe_json_for_script\`: make JSON safe as raw text inside a script. */
function __ruvyxaSafeJson(json) {
  return json
    .split("<").join("\\\\u003c")
    .split(">").join("\\\\u003e")
    .split("&").join("\\\\u0026")
    .split("\\u2028").join("\\\\u2028")
    .split("\\u2029").join("\\\\u2029")
}

/** Twin of \`escape_html\`, for an attribute value. */
function __ruvyxaEscapeAttribute(value) {
  return String(value)
    .split("&").join("&amp;")
    .split("<").join("&lt;")
    .split(">").join("&gt;")
    .split('"').join("&quot;")
}

/** Twin of \`url_encode_component\`. */
function __ruvyxaEncodeComponent(value) {
  let out = ""
  for (const byte of new TextEncoder().encode(String(value))) {
    const char = String.fromCharCode(byte)
    if (/[A-Za-z0-9\\-_.~/]/.test(char)) out += char
    else out += "%" + byte.toString(16).toUpperCase().padStart(2, "0")
  }
  return out
}

/** Twin of \`bootstrap_data_block\`. */
function __ruvyxaBootstrapBlock(params, requestPath, csr) {
  const payload = { params: params ?? {}, path: requestPath }
  // Absent rather than false, so a hydrating page carries no CSR marker.
  if (csr === true) payload.csr = true
  let json
  try {
    json = JSON.stringify(payload)
  } catch {
    // An unserializable params object loses the parameters, not the document.
    json = JSON.stringify({ params: {}, path: requestPath })
  }
  return '<script type="application/json" id="' + __RUVYXA_BOOTSTRAP_ID + '">' +
    __ruvyxaSafeJson(json) + "</script>"
}

/**
 * Twin of \`rsc_payload_block\`.
 *
 * Quoted as a JSON *string* rather than embedded raw: a Flight payload is a
 * line-delimited format, not a JSON document, and quoting is what lets the same
 * escaping every other block uses apply to it unchanged.
 */
function __ruvyxaRscPayloadBlock(payload) {
  return '<script type="application/json" id="' + __RUVYXA_RSC_ID + '">' +
    __ruvyxaSafeJson(JSON.stringify(String(payload))) + "</script>"
}

/** Twin of \`hydration_loader_url\`. */
function __ruvyxaHydrationSrc(assets) {
  if (!assets.hydrationLoader) return assets.src
  if (assets.hydration !== "idle" && assets.hydration !== "visible") return assets.src
  return assets.hydrationLoader + "?strategy=" + assets.hydration +
    "&src=" + __ruvyxaEncodeComponent(assets.src)
}

/**
 * Insert head and tail fragments into a rendered document.
 *
 * Twin of the placement in \`inject_prerender_client_assets\`: preloads before
 * \`</head>\`, scripts before \`</body>\`, and a whole document synthesised when
 * the render produced a fragment rather than a page.
 *
 * The two ends are placed independently, because a rendered document does not
 * always have both. A root layout that returns \`<html><body>…</body></html>\`
 * with no \`<head>\` used to fall all the way through to the synthesise branch
 * and be wrapped in a second \`<html>\` and \`<body>\` — valid enough that a
 * browser recovers, and wrong enough that the document it parsed was not the
 * one the application wrote. A head is inserted before the body it already has
 * instead.
 */
function __ruvyxaInjectDocumentAssets(html, head, tail) {
  // Twin of \`document_head_defaults\`: without a viewport declaration a phone
  // lays the page out at 980px and scales it down, so every breakpoint in the
  // application is evaluated against a width no device has. A document that
  // declares its own keeps it.
  const viewport =
    /name=["']viewport["']/i.test(html)
      ? ""
      : '<meta name="viewport" content="width=device-width, initial-scale=1">'
  head = viewport + head
  if (head === "" && tail === "") return html
  const lower = html.toLowerCase()
  const headEnd = lower.indexOf("</head>")
  const bodyStart = lower.indexOf("<body")
  if (headEnd !== -1) {
    html = html.slice(0, headEnd) + head + html.slice(headEnd)
  } else if (bodyStart !== -1) {
    html = html.slice(0, bodyStart) + "<head>" + head + "</head>" + html.slice(bodyStart)
  } else {
    return "<!doctype html><html><head>" + head + "</head><body>" + html + tail + "</body></html>"
  }
  const bodyEnd = html.toLowerCase().lastIndexOf("</body>")
  if (bodyEnd === -1) return html + tail
  return html.slice(0, bodyEnd) + tail + html.slice(bodyEnd)
}

/**
 * The head and tail one page render contributes.
 *
 * \`assets\` is null for a route that ships no client bundle —
 * \`export const hydrate = false\` — and then only an RSC payload and the
 * stylesheet can be added. A deferred bundle emits no preloads, matching both
 * Rust writers: preloading a module the page has decided not to run yet is work
 * for nothing.
 *
 * The stylesheet link is added whatever the route ships, because a deployed
 * function has no \`app/\` to compile CSS from and no collector to run: without
 * it every request-time render on a deployed build reached the browser
 * unstyled, while the pre-rendered pages beside it looked correct.
 */
function __ruvyxaDocumentAssets(assets, ctx, rscPayload) {
  const styles = __RUVYXA_STYLE_HEAD
  if (!assets) {
    return {
      head: styles,
      tail: rscPayload == null ? "" : __ruvyxaRscPayloadBlock(rscPayload),
    }
  }
  const deferred = assets.hydration === "idle" || assets.hydration === "visible"
  const head = styles + (deferred
    ? ""
    : (assets.preloads ?? [])
        .map((src) => '<link rel="modulepreload" href="' + __ruvyxaEscapeAttribute(src) + '">')
        .join(""))
  const payload = rscPayload == null ? "" : __ruvyxaRscPayloadBlock(rscPayload)
  const bootstrap = __ruvyxaBootstrapBlock(ctx.params ?? {}, ctx.path, false)
  const script =
    '<script type="module" src="' + __ruvyxaEscapeAttribute(__ruvyxaHydrationSrc(assets)) + '"></script>'
  return { head, tail: payload + bootstrap + script }
}`
}

/** Global registry of route pattern to tree factory, read by the client router. */
export const ROUTE_REGISTRY_GLOBAL = '__RUVYXA_ROUTES__'
const SHELL_REGISTRY_GLOBAL = '__RUVYXA_SHELLS__'

/**
 * Global registry of route pattern to the URLs that route can intercept.
 *
 * Separate from the tree registry because the router reads it *before* it
 * decides whether a navigation is a route change at all: an intercepted URL
 * never swaps the mounted route.
 */
export const INTERCEPT_REGISTRY_GLOBAL = '__RUVYXA_INTERCEPTS__'

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

/** Local name the emitted prelude binds the interception-aware slot resolver to. */
export const ROUTE_SLOT_LOCAL = '__ruvyxaSlot'

/**
 * Global registry of route pattern to a server-components tree factory.
 *
 * Separate from {@link ROUTE_REGISTRY_GLOBAL} because the two answer different
 * shapes: an ordinary factory takes a route context and returns a tree it can
 * build on the spot, while this one also needs the Flight payload the route was
 * rendered from. One registry holding both would make "which kind of route is
 * this" a guess at every call site that reads it.
 */
export const RSC_ROUTE_REGISTRY_GLOBAL = '__RUVYXA_RSC_ROUTES__'

/** Local name the server-components entry binds its root component to. */
export const RSC_ROOT_LOCAL = '__ruvyxaRscRoot'

/** Local name the server-components entry binds its registered factory to. */
export const RSC_TREE_LOCAL = '__ruvyxaRscTree'

/**
 * The slot resolver a route with interceptions emits.
 *
 * A slot normally renders one fixed component. An interception replaces that
 * content for as long as the URL names it, while the page underneath stays
 * mounted — so the slot has to be decided per render rather than baked in.
 * `ctx.intercept` is set by the client router and is absent on the server and
 * on a hard load, which is what makes a refresh show the real page.
 *
 * A slot may carry an interception without having a default of its own, so
 * `Default` is nullable rather than assumed.
 *
 * Mirrors `ROUTE_SLOT_PRELUDE` in `crates/ruvyxa_bundler/src/output.rs`.
 */
export const ROUTE_SLOT_PRELUDE = `function ${'__ruvyxaSlot'}(ctx, level, name, Default, intercepts) {
  const active = ctx.intercept
  if (active && active.level === level && active.name === name) {
    for (const entry of intercepts) {
      if (entry[0] === active.target) {
        return React.createElement(entry[1], {
          params: active.params ?? {},
          requestPath: active.path ?? ctx.path,
        })
      }
    }
  }
  return Default
    ? React.createElement(Default, { params: ctx.params ?? {}, requestPath: ctx.path })
    : null
}
`

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
    // Ask the server for this route again, then clear the boundary.
    // A plain reset re-renders the payload that just failed, so it can only
    // recover from a fault in the client tree. A page whose data failed to load
    // needs the request repeated, which is what the router's retry does.
    // Without a mounted router there is nothing to re-fetch from, so this
    // degrades to a plain reset rather than doing nothing at all.
    this.retry = () => {
      const router = globalThis.__RUVYXA_ROUTER_INSTANCE__
      if (!router || typeof router.retry !== "function") {
        this.reset()
        return Promise.resolve()
      }
      return Promise.resolve(router.retry()).then(
        () => this.reset(),
        (failure) => this.setState({ error: failure }),
      )
    }
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
        return React.createElement(this.props.errorFallback, {
          error,
          reset: this.reset,
          retry: this.retry,
        })
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
 * @param {string[]} [options.metaNames] Namespace identifiers from
 *   {@link metaSourceImports}, least specific first. Omitting them emits a tree
 *   with no metadata code at all, which is what a caller that has not adopted
 *   route metadata gets.
 */
/**
 * Interleave the layout and template chains into one root-first level list.
 *
 * Both chains are ordered root-first and each entry names a file, so the
 * directory holding it is the level. Merging on that directory is what keeps
 * `layout > template` correct at every level even when only one of the two
 * exists there — flattening it into "every template inside every layout" would
 * put a layout outside a template that should have contained it, which is
 * observable the moment a template provides context.
 *
 * Mirrors `route_wrapper_levels()` in `crates/ruvyxa_bundler/src/output.rs`.
 *
 * @param {string[]} layoutPaths Layout files, root first.
 * @param {string[]} templatePaths `template.tsx` files, root first.
 * @returns {{ layout: string|null, template: string|null }[]}
 */
export function wrapperLevels(layoutPaths = [], templatePaths = [], slots = [], intercepts = []) {
  const normalize = (value) => value.replace(/\\/g, '/').replace(/\/+$/, '')
  const directory = (file) => normalize(file).replace(/\/[^/]*$/, '')
  const levels = new Map()
  const at = (key) => {
    if (!levels.has(key)) {
      levels.set(key, { layout: null, template: null, slots: null, intercepts: [] })
    }
    return levels.get(key)
  }

  layoutPaths.forEach((file, index) => {
    at(directory(file)).layout = `Layout${index}`
  })
  templatePaths.forEach((file, index) => {
    at(directory(file)).template = `Template${index}`
  })
  // A slot names the directory holding its `@name` folder, which is the level
  // whose layout receives it — the same key the two chains merge on.
  slots.forEach((slot, index) => {
    const level = at(normalize(slot.level))
    level.slots ??= {}
    level.slots[slot.name] = `Slot${index}`
  })
  // An interception merges on the same key a slot does: it replaces that slot's
  // content while the page underneath stays mounted, so it belongs to the level
  // whose layout receives the prop. `levelDir` is only the merge key; `level` in
  // the emitted entry is the route id both hosts spell the same way.
  intercepts.forEach((intercept, index) => {
    at(normalize(intercept.levelDir)).intercepts.push({
      name: intercept.name,
      level: intercept.levelId,
      target: intercept.target,
      component: `Intercept${index}`,
    })
  })

  // Root-first. A shorter directory is always an ancestor here, because every
  // entry lies on one path from the app root to the route.
  return [...levels.entries()]
    .sort((left, right) => left[0].split('/').length - right[0].split('/').length)
    .map(([, level]) => level)
}

/**
 * Emit the loop that wraps a tree in its layouts and templates.
 *
 * A route with no `template.tsx` emits exactly the loop it always did: nothing
 * about an ordinary route's bundle changes because the feature exists.
 *
 * The template's `key` is the whole reason the file exists: React remounts a
 * keyed element when the key changes, so navigating within the same layout
 * resets the template's state and re-runs its effects, while the layout above
 * it stays mounted.
 *
 * Mirrors `wrapper_loop()` in `crates/ruvyxa_bundler/src/output.rs`; both are
 * pinned by `tests/fixtures/entry-composition-conformance.json`.
 *
 * @param {string[]} layoutNames Layout identifiers, root-to-leaf.
 * @param {{ layout: string|null, template: string|null }[]} levels
 */
/**
 * Import statements and composition levels for a route's layouts and templates.
 *
 * The two interleave by directory, so building the identifiers and the levels
 * together is what keeps `Layout0`/`Template0` pointing at the files
 * `wrapperLevels()` names.
 *
 * Lives here rather than in `worker-pool.mjs` because two hosts now generate a
 * server-components entry — that worker for `ruvyxa dev`/`start`, and
 * `adapter-runner.mjs` for a deployed function — and a route whose wrappers are
 * numbered differently in the two produces a document that hydrates against a
 * tree it does not match.
 */
export function wrapperEntryParts(layouts, templates, slots = [], intercepts = []) {
  const imports = []
  const layoutNames = []
  layouts.forEach((file, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(file))}`)
    layoutNames.push(`Layout${index}`)
  })
  templates.forEach((file, index) => {
    imports.push(`import Template${index} from ${JSON.stringify(toImportPath(file))}`)
  })
  slots.forEach((slot, index) => {
    imports.push(`import Slot${index} from ${JSON.stringify(toImportPath(slot.file))}`)
  })
  intercepts.forEach((intercept, index) => {
    imports.push(`import Intercept${index} from ${JSON.stringify(toImportPath(intercept.file))}`)
  })
  return {
    imports,
    layoutNames,
    levels: wrapperLevels(layouts, templates, slots, intercepts),
  }
}

export function wrapperLoop(layoutNames, levels = []) {
  if (levels.every((level) => !level.template && !hasSlots(level) && !hasIntercepts(level))) {
    return `  for (const Layout of [${layoutNames.join(', ')}].reverse()) {
    tree = React.createElement(Layout, null, tree)
  }`
  }
  const triples = levels
    .map((level) => `[${level.layout ?? 'null'}, ${level.template ?? 'null'}, ${slotProps(level)}]`)
    .join(', ')
  return `  for (const [Layout, Template, slots] of [${triples}].reverse()) {
    if (Template) tree = React.createElement(Template, { key: ctx.path }, tree)
    if (Layout) tree = React.createElement(Layout, slots, tree)
  }`
}

function hasSlots(level) {
  return Boolean(level.slots) && Object.keys(level.slots).length > 0
}

function hasIntercepts(level) {
  return Array.isArray(level.intercepts) && level.intercepts.length > 0
}

/**
 * The slot props one level's layout receives, or `null`.
 *
 * Built inside the loop rather than hoisted, because the elements depend on
 * `ctx` and the loop runs once per render. Ordered by slot name so the emitted
 * source does not depend on the order a filesystem listed the directories in.
 */
function slotProps(level) {
  if (!hasSlots(level) && !hasIntercepts(level)) return 'null'
  const defaults = level.slots ?? {}
  const intercepts = level.intercepts ?? []
  const names = [...new Set([...Object.keys(defaults), ...intercepts.map((entry) => entry.name)])]
  names.sort(compareCodeUnits)
  const props = names
    .map((name) => {
      const matching = intercepts.filter((entry) => entry.name === name)
      // A slot nothing can intercept emits exactly the element it always did,
      // so a project using parallel routes and nothing else keeps its bundle.
      if (matching.length === 0) {
        return `${JSON.stringify(name)}: React.createElement(${defaults[name]}, { params: ctx.params ?? {}, requestPath: ctx.path })`
      }
      const table = matching
        .map((entry) => `[${JSON.stringify(entry.target)}, ${entry.component}]`)
        .join(', ')
      return `${JSON.stringify(name)}: ${ROUTE_SLOT_LOCAL}(ctx, ${JSON.stringify(matching[0].level)}, ${JSON.stringify(name)}, ${defaults[name] ?? 'null'}, [${table}])`
    })
    .join(', ')
  return `{ ${props} }`
}

/**
 * Publish what this route can intercept, for the client router to match.
 *
 * Only the metadata travels: the router needs to know *whether* a URL is
 * intercepted from here, and the component that answers it is already in this
 * bundle behind the emitted slot resolver.
 *
 * Mirrors `intercept_registry_statement()` in
 * `crates/ruvyxa_bundler/src/output.rs`.
 */
export function interceptRegistryStatement(routePath, intercepts = []) {
  if (intercepts.length === 0) return ''
  const entries = intercepts
    .map(
      (intercept) =>
        `{ level: ${JSON.stringify(intercept.level)}, name: ${JSON.stringify(intercept.name)}, target: ${JSON.stringify(intercept.target)} }`,
    )
    .join(', ')
  return `;(globalThis.${INTERCEPT_REGISTRY_GLOBAL} ||= {})[${JSON.stringify(routePath)}] = [${entries}];`
}

export function routeTreeFunction({
  name,
  pageName,
  layoutNames,
  routePath,
  metaNames = [],
  errorName = null,
  loadingName = null,
  notFoundName = null,
  levels = [],
  provider = true,
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
  lines.push(wrapperLoop(layoutNames, levels))
  // Metadata is a sibling of the layouts, not a wrapper around them: a layout
  // that suspends must not be able to hold the document title back past the
  // flushed shell. It is passed as an extra child of the provider — an element
  // array with its own keys — so no extra wrapper element is created per render.
  const metaChild =
    metaNames.length > 0
      ? `${META_ELEMENT_LOCAL}(${META_RESOLVE_LOCAL}([${metaNames.join(', ')}], ctx)), `
      : ''
  // A server-components graph has no `React.createContext`: the react-server
  // build does not export it, because a context read is a client concern. The
  // provider is emitted by whichever graph hydrates the tree instead — the SSR
  // pass and the browser entry both wrap the decoded element in it, so the
  // markup they produce still matches.
  if (provider) {
    lines.push(`  return React.createElement(${ROUTE_CONTEXT_LOCAL}.Provider, {
    value: { pathname: ctx.path, params: ctx.params ?? {}, route: ${JSON.stringify(routePath)}, flight: ctx.flight },
  }, ${metaChild}tree)`)
  } else if (metaChild) {
    lines.push(`  return React.createElement(React.Fragment, null, ${metaChild}tree)`)
  } else {
    lines.push('  return tree')
  }
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
 * Emit the route's loading shell: its layouts wrapped around `loading.tsx`.
 *
 * This is the half of a route that needs no server data. The layouts and the
 * loading component are already in the route bundle, so once that bundle is
 * warm — which is exactly what `<Link>` prefetching does — the client can paint
 * the shell with no request at all.
 *
 * That is what makes a navigation feel instant. Without it the router holds the
 * previous page on screen until the Flight payload arrives, so a slow route
 * looks like a dead click; with it the user sees the destination's own chrome
 * and its loading state immediately, and the content replaces the fallback when
 * the payload lands.
 *
 * Emitted only for a route that has a `loading.tsx`. A route without one has no
 * declared loading state, and inventing a blank screen for it would be worse
 * than leaving the previous page up.
 *
 * @param {object} options
 * @param {string} options.name Function name to declare.
 * @param {string[]} options.layoutNames Layout identifiers, root-to-leaf.
 * @param {string} options.routePath Route pattern, e.g. `/blog/[slug]`.
 * @param {string} options.loadingName `loading.tsx` component identifier.
 * @param {string[]} [options.metaNames] Namespace identifiers, least specific first.
 */
export function routeShellFunction({
  name,
  layoutNames,
  routePath,
  loadingName,
  metaNames = [],
  levels = [],
}) {
  const lines = [`  let tree = React.createElement(${loadingName}, null)`]
  lines.push(wrapperLoop(layoutNames, levels))
  const metaChild =
    metaNames.length > 0
      ? `${META_ELEMENT_LOCAL}(${META_RESOLVE_LOCAL}([${metaNames.join(', ')}], ctx)), `
      : ''
  lines.push(`  return React.createElement(${ROUTE_CONTEXT_LOCAL}.Provider, {
    value: { pathname: ctx.path, params: ctx.params ?? {}, route: ${JSON.stringify(routePath)}, flight: undefined },
  }, ${metaChild}tree)`)
  return `function ${name}(ctx) {\n${lines.join('\n')}\n}`
}

/**
 * Register the shell so the client router can paint it during a navigation.
 *
 * Kept in a registry separate from `__RUVYXA_ROUTES__` because a route may have
 * a tree and no shell, and the router has to be able to tell the difference
 * rather than rendering a tree that would immediately suspend on missing data.
 */
export function routeShellRegistration({ name, routePath }) {
  return `;(globalThis.${SHELL_REGISTRY_GLOBAL} ||= {})[${JSON.stringify(routePath)}] = ${name}`
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
  intercepts = [],
  requestPathLiteral,
  paramsLiteral,
  errorName = null,
  loadingName = null,
  notFoundName = null,
  metaNames = [],
  levels = [],
}) {
  const boundary = needsRouteBoundary({ errorName, notFoundName })
    ? `\n${routeBoundaryPrelude()}\n`
    : ''
  const meta = metaNames.length > 0 ? `\n${routeMetaPrelude({ lang: false })}\n` : ''
  // Only a route with a declared loading state gets a shell; see
  // `routeShellFunction`.
  const shell = loadingName
    ? `${routeShellFunction({ name: '__ruvyxaShell', layoutNames, routePath, loadingName, metaNames, levels })}\n${routeShellRegistration({ name: '__ruvyxaShell', routePath })}\n`
    : ''
  // A route with no interceptions emits neither the resolver nor the table, so
  // a project that has never heard of the convention keeps the bundle it had.
  const slotResolver = intercepts.length > 0 ? `\n${ROUTE_SLOT_PRELUDE}` : ''
  const interceptRegistry = interceptRegistryStatement(routePath, intercepts)
  return `import React from "react"
import { hydrateRoot } from "react-dom/client"
${imports.join('\n')}

${routeContextPrelude()}
${boundary}${slotResolver}${meta}
${routeTreeFunction({ name: '__ruvyxaTree', pageName, layoutNames, routePath, errorName, loadingName, notFoundName, metaNames, levels })}
${routeRegistration({ name: '__ruvyxaTree', routePath })}
${interceptRegistry}
${shell}

${clientBootstrapPrelude()}

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
 * Build a server SSR entry that streams through `renderToPipeableStream`, or
 * through `renderToReadableStream` on Bun and Deno, whose `react-dom/server`
 * has no pipeable renderer. Both buffer the stream into the document string
 * this entry returns; the streaming renderer is what lets a component await.
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
  levels = [],
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
${routeTreeFunction({ name: '__ruvyxaTree', pageName, layoutNames, routePath, errorName, loadingName, notFoundName, metaNames, levels })}
${recovery}
export async function render(ctx) {
  const html = await __ruvyxaRenderDocument(ctx)
  return ${applyLang}
}
${flight}

async function __ruvyxaRenderDocument(ctx) {
  const tree = __ruvyxaTree(ctx)

  if (typeof ReactDomServer.renderToPipeableStream !== "function") {
    return __ruvyxaRenderWebStream(ctx, tree)
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

/**
 * Render the document on a runtime whose \`react-dom/server\` is a web build.
 *
 * Bun and Deno resolve that specifier to an entry point exporting
 * \`renderToReadableStream\` and no \`renderToPipeableStream\`. \`renderToString\`
 * is not the substitute it looks like: it is the synchronous legacy renderer,
 * and a component that awaits anything — every async server component — makes
 * it throw "A component suspended while responding to synchronous input"
 * instead of rendering. It stays here as the last resort for a runtime that
 * offers neither streaming renderer.
 */
async function __ruvyxaRenderWebStream(ctx, tree) {
  if (typeof ReactDomServer.renderToReadableStream !== "function") {
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

  let captured = null
  let html = null
  try {
    const stream = await ReactDomServer.renderToReadableStream(tree, {
      onError(error) {
        if (!captured) captured = error
      },
    })
${
  readyEvent === 'onAllReady'
    ? `    // Nothing is read until the whole render is done, which is what makes
    // the buffered document hold finished markup rather than a fallback and
    // the inline script that replaces it. The pipeable path above says the
    // same thing by piping from \`onAllReady\`.
    await stream.allReady
`
    : ''
}    html = await new Response(stream).text()
  } catch (error) {
    if (!captured) captured = error
  }

  if (captured) {${
    serverRecovers
      ? `
    const recovery = __ruvyxaRecovery(ctx, captured)
    if (recovery) return "<!doctype html>" + ReactDomServer.renderToString(recovery)`
      : ''
  }
${
  tolerateStreamErrors || serverRecovers
    ? `    if (globalThis.process?.env?.RUVYXA_DEBUG) console.error("[ruvyxa] streaming render error", captured)
    // A slot that threw is survivable; a shell that never produced a document
    // is not, and there is nothing to return in its place.
    if (html === null) throw captured`
    : '    throw captured'
}
  }

  return html.trimStart().toLowerCase().startsWith("<!doctype") ? html : "<!doctype html>" + html
}
`
}

/**
 * Build the server-components entry: a module that renders a route's tree into
 * a Flight payload.
 *
 * This is the only generated entry compiled with the `react-server` export
 * condition, so it is the only one whose `React` has no `createContext`, no
 * `useState`, and no class-component lifecycle. Three things that every other
 * entry emits are therefore absent here, and each absence is a rule rather than
 * an omission:
 *
 * - **No routing context provider.** `React.createContext` does not exist in
 *   this graph. The provider is emitted by the two graphs that consume the
 *   payload instead — {@link rscClientEntrySource} and the SSR pass — which
 *   both wrap the decoded element in it, so the markup still matches.
 * - **No error boundary.** `routeBoundaryPrelude` is a class component, and a
 *   server graph has no class lifecycle. An `error.tsx` on a server-components
 *   route has to be a `'use client'` module, which is the same rule React
 *   itself imposes.
 * - **No document assembly.** The payload is not HTML. Turning it into HTML is
 *   the SSR pass's job, and it runs with the ordinary React.
 *
 * `loading.tsx` *is* supported: `React.Suspense` exists in both graphs.
 *
 * There is no Rust mirror of this function. `crates/ruvyxa_bundler/src/output.rs`
 * mirrors the entries the Rust build path emits, and that path has no
 * server-components target yet; when it gains one, this shape gains a second
 * implementation and needs a shared fixture on that day.
 *
 * @param {object} options
 * @param {string[]} options.imports Import statements for page and layouts.
 * @param {string} options.pageName Identifier the page component is bound to.
 * @param {string[]} options.layoutNames Layout identifiers, root-to-leaf.
 * @param {string} options.routePath Route pattern, e.g. `/blog/[slug]`.
 * @param {string|null} [options.loadingName] `loading.tsx` component identifier.
 * @param {string[]} [options.metaNames] Namespace identifiers from {@link metaSourceImports}.
 * @param {{ layout: string|null, template: string|null }[]} [options.levels]
 */
export function rscServerEntrySource({
  imports,
  pageName,
  layoutNames,
  routePath,
  loadingName = null,
  metaNames = [],
  levels = [],
}) {
  const metaPrelude = metaNames.length > 0 ? `\n${routeMetaPrelude({ lang: false })}\n` : ''
  const tree = routeTreeFunction({
    name: '__ruvyxaTree',
    pageName,
    layoutNames,
    routePath,
    loadingName,
    metaNames,
    levels,
    provider: false,
  })
  return `import React from "react"
import { renderToReadableStream as __ruvyxaRenderFlight } from ${JSON.stringify(RSC_SERVER_PACKAGE)}
import { flushServerModules as __ruvyxaFlushServer } from ${JSON.stringify(RSC_CLIENT_RUNTIME_SPECIFIER)}
${imports.join('\n')}
${metaPrelude}
${tree}

export function flight(ctx, manifest, options) {
  // Before the first serialisation, never at module scope: a "use server"
  // module enqueues itself at the bottom of its own body, and this linker emits
  // that module's export assignments after the body — so its exports are only
  // readable once the whole bundle has evaluated, which is here.
  __ruvyxaFlushServer()
  return __ruvyxaRenderFlight(__ruvyxaTree(ctx), manifest, options)
}
`
}

/**
 * Build the browser entry for a server-components route.
 *
 * Unlike {@link clientEntrySource} this entry does **not** import the page: on
 * a server-components route the page never reaches the browser, which is the
 * point. What it imports is the set of `'use client'` modules the server graph
 * turned into references, so `__webpack_require__` can answer for each id, plus
 * the decoder that turns a payload into an element tree.
 *
 * It publishes that tree twice over. Once immediately, against the payload the
 * document was served with, to hydrate. And once into
 * `__RUVYXA_RSC_ROUTES__` — a registry keyed by route pattern, which the client
 * router calls with a payload it fetched, so a navigation *into* this route
 * replaces the tree instead of reloading the document. The registry is separate
 * from `__RUVYXA_ROUTES__` because those factories are synchronous and take no
 * payload; one registry answering two shapes would make "which kind of route is
 * this" a guess at the call site.
 *
 * The tree is wrapped in the same routing-context provider the SSR pass wraps it
 * in, with the same value, so hydration sees the markup it rendered.
 *
 * A document served without a payload — a route that opted in but was rendered
 * by an older server, or a response an error replaced — leaves the page as
 * served rather than blanking it: `hydrateRoot` with nothing to hydrate would
 * throw away server-rendered HTML the visitor is already reading.
 *
 * @param {object} options
 * @param {{ id: string, file: string }[]} options.references Client modules to register.
 * @param {string} options.routePath Route pattern for the routing context.
 * @param {string} options.requestPathLiteral JS literal for the fallback path.
 * @param {string} options.paramsLiteral JS literal for the fallback params.
 */
export function rscClientEntrySource({ references, routePath, requestPathLiteral, paramsLiteral }) {
  // By absolute path rather than by alias: this entry is compiled by two
  // bundlers — this package's during `ruvyxa dev`, the Rust one during
  // `ruvyxa build` — and only the first knows the alias. Both bundle an
  // absolute path into a browser target.
  const runtimePath = clientReferenceRuntimePath()
  const registry = clientRegistrySource(references, runtimePath)
  // First, and the only import here whose *position* matters: importing it
  // installs the globals `react-server-dom-webpack/client.browser` reads while
  // its own module body runs.
  return `import ${JSON.stringify(clientReferenceInstallPath())}
${registry.imports.join('\n')}
import { payloadStream as __ruvyxaPayloadStream, readInlinePayload as __ruvyxaReadPayload } from ${JSON.stringify(runtimePath)}
import React from "react"
import { hydrateRoot } from "react-dom/client"
import { createFromReadableStream as __ruvyxaDecodeFlight, createFromFetch as __ruvyxaFromFetch, encodeReply as __ruvyxaEncodeReply } from ${JSON.stringify(RSC_BROWSER_PACKAGE)}
import { createServerCaller as __ruvyxaServerCaller } from ${JSON.stringify(runtimePath)}

${registry.statements.join('\n')}

${routeContextPrelude()}

${clientBootstrapPrelude()}

// One decode per payload. React's decoder consumes the stream it is given, so
// calling it again for a re-render would hand React a reader that is already
// drained. Only the newest payload is kept: an older one belongs to a route
// that is no longer mounted.
let __ruvyxaDecodedPayload = null
let __ruvyxaDecodedResponse = null
// A server function reaches the browser two ways: imported from a "use server"
// module, where the proxy carries its own caller, and inside a payload, where
// the decoder mints the reference and calls whatever this option names. Both
// end up in the same place; without this one React throws "the callServer
// option was not implemented in your router runtime" the first time a page
// passes a server function down as a prop.
const __ruvyxaCallServer = __ruvyxaServerCaller({
  encodeReply: __ruvyxaEncodeReply,
  createFromFetch: __ruvyxaFromFetch,
})

function __ruvyxaResponseFor(payload) {
  if (payload !== __ruvyxaDecodedPayload) {
    __ruvyxaDecodedPayload = payload
    __ruvyxaDecodedResponse = __ruvyxaDecodeFlight(__ruvyxaPayloadStream(payload), {
      callServer: __ruvyxaCallServer,
    })
  }
  return __ruvyxaDecodedResponse
}

// A component declared once, at module scope. Building it inside the factory
// below would make every render a new component *type*, which React unmounts
// and remounts — losing the state of every client component on the page.
function ${RSC_ROOT_LOCAL}({ payload, path, params }) {
  return React.createElement(
    ${ROUTE_CONTEXT_LOCAL}.Provider,
    { value: { pathname: path, params: params ?? {}, route: ${JSON.stringify(routePath)}, flight: undefined } },
    React.use(__ruvyxaResponseFor(payload)),
  )
}

function ${RSC_TREE_LOCAL}(payload, ctx) {
  return React.createElement(${RSC_ROOT_LOCAL}, {
    payload,
    path: ctx.path,
    params: ctx.params,
  })
}

;(globalThis.${RSC_ROUTE_REGISTRY_GLOBAL} ||= {})[${JSON.stringify(routePath)}] = ${RSC_TREE_LOCAL};

const __ruvyxaCtx = {
  path: globalThis.__RUVYXA_REQUEST_PATH__ ?? ${requestPathLiteral},
  params: globalThis.__RUVYXA_ROUTE_PARAMS__ ?? ${paramsLiteral},
}
globalThis.${ROUTE_PATTERN_GLOBAL} = ${JSON.stringify(routePath)}
const __ruvyxaPayload = __ruvyxaReadPayload()

if (__ruvyxaPayload !== null) {
  const __ruvyxaElement = ${RSC_TREE_LOCAL}(__ruvyxaPayload, __ruvyxaCtx)
  if (globalThis.__RUVYXA_ROOT__) {
    globalThis.__RUVYXA_ROOT__.render(__ruvyxaElement)
  } else {
    globalThis.__RUVYXA_ROOT__ = hydrateRoot(document, __ruvyxaElement)
  }
  window.__RUVYXA_HYDRATED = true
}
`
}

/**
 * Build the entry that runs one of a route's server functions.
 *
 * A `POST` to `/__ruvyxa/rsc` names a reference; this module is what turns that
 * name back into a call. It is separate from the route's render entry for one
 * reason: an actions file imported only by a `'use client'` component is not in
 * the render entry's graph at all — the component is a reference there, and a
 * reference's own imports are never walked — so the render entry could not
 * resolve half the functions a page can call.
 *
 * Every `'use server'` module either graph reported is imported, not just the
 * one the call names. React resolves a reference that appears *inside* the
 * arguments through the same registry, which is how `remove.bind(null, id)`
 * reaches the server, and a bundle holding only the called module would fail on
 * the first such argument.
 *
 * Decoding, calling, and re-encoding all happen here rather than in the host
 * because all three need the `react-server` build of React and of
 * `react-server-dom-webpack`: the host process has the ordinary one, and a
 * reply encoded by the wrong instance names references the caller cannot
 * resolve.
 *
 * @param {object} options
 * @param {{ id: string, file: string }[]} options.references `'use server'` modules to link.
 */
export function rscActionEntrySource({ references }) {
  const imports = references.map(
    (reference, index) =>
      `import * as __ruvyxaAction${index} from ${JSON.stringify(String(reference.file).replaceAll('\\', '/'))}`,
  )
  // Referenced so a tree-shaking pass cannot decide the namespaces are unused
  // and drop the modules whose evaluation is the entire point of importing them.
  const touched = references.map((_reference, index) => `__ruvyxaAction${index}`).join(', ')
  return `import { decodeAction, decodeFormState, decodeReply, renderToReadableStream as __ruvyxaRenderFlight } from ${JSON.stringify(RSC_SERVER_PACKAGE)}
import { flushServerModules as __ruvyxaFlushServer, installClientReferenceRuntime as __ruvyxaInstallRefs, resolveServerReference as __ruvyxaResolveServer } from ${JSON.stringify(RSC_CLIENT_RUNTIME_SPECIFIER)}
${imports.join('\n')}

export const linkedModules = [${touched}]

/**
 * Call the function \`reference\` names and encode what it returned.
 *
 * The reply is a Flight payload rather than JSON so a server function may
 * return an element tree — including client components, which is why it needs
 * the same manifest a page render uses.
 */
export async function callServerFunction({ reference, body, manifest, serverManifest, options }) {
  __ruvyxaFlushServer()
  const target = __ruvyxaResolveServer(reference)
  const args = await decodeReply(body, serverManifest, options)
  const returnValue = await target(...args)
  return __ruvyxaRenderFlight(returnValue, manifest, options)
}

/**
 * Run the action a plain form post named, for a browser with no JavaScript.
 *
 * React writes the reference and its bound arguments into hidden fields when it
 * renders \`<form action={fn}>\`, so the submission carries everything needed to
 * find and call the function. \`decodeAction\` reads those fields and hands back
 * a thunk; the page is then re-rendered normally, which is what makes the
 * result visible without a payload ever reaching the browser.
 *
 * \`null\` when the post named no action. That is the ordinary answer for any
 * other form on the page — a search box posting to the same URL, say — and the
 * caller renders the route as it would have without a body.
 *
 * The second half is \`useActionState\`. React writes an extra \`$ACTION_KEY\`
 * field for a form whose action came from that hook, and \`decodeFormState\`
 * turns the return value into the token the HTML renderer replays it from. Any
 * other form decodes to \`null\` here and the hook keeps its initial state,
 * which is the same thing the hook does on a first load.
 */
export async function runFormAction({ formData, serverManifest }) {
  __ruvyxaFlushServer()
  // \`decodeAction\` resolves the reference itself, through the same
  // \`__webpack_require__\` a payload resolves a client component with — a
  // hidden field is decoded by React, not by this framework, so the id in it
  // takes React's route rather than \`resolveServerReference\`'s. Installing
  // here rather than at module scope keeps the claim deliberate, which is what
  // the runtime's own comment asks for.
  __ruvyxaInstallRefs()
  const action = await decodeAction(formData, serverManifest)
  if (typeof action !== 'function') return null
  const result = await action()
  return { result, formState: await decodeFormState(result, formData, serverManifest) }
}
`
}
