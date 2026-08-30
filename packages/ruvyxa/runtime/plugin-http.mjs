/**
 * Plugin registry construction and HTTP hook dispatch.
 *
 * Extracted from `plugin-runtime.mjs`, which could only ever run as a child
 * process of the Rust host: it speaks NDJSON over stdio and is spawned from
 * `ruvyxa_dev_server` and `ruvyxa_cli`. Nothing in a deployed build spawned it,
 * so `http.onRequest`, `http.onResponse`, and `http.route` — and therefore the
 * whole of `@ruvyxa/auth`, which is a single `http.onRequest` registration —
 * existed under `ruvyxa dev` and `ruvyxa start` and returned 404 everywhere
 * else.
 *
 * Splitting the registry out of the transport is what makes a second host
 * possible: `plugin-runtime.mjs` keeps the stdio protocol and wraps these
 * functions, while a function bundle imports them directly and runs the same
 * hooks against native `Request`/`Response` objects with no base64 round-trip.
 *
 * Deliberately free of `node:` imports so it can be compiled into an edge
 * bundle alongside the plugins themselves.
 */

import { canonicalRoutePath } from './route-match.mjs'

/**
 * Whether the fetch specification says this status carries no body.
 *
 * `new Response(body, { status })` throws for any of them unless the body is
 * exactly `null`, and a zero-length body is not null. Every response hook the
 * documentation shows rebuilds the response as
 * `new Response(response.body, { status, headers })` — so a host that encoded
 * "no body" as an empty string handed the plugin a Response it could not
 * rebuild, and a project with any response hook answered 500 for every 204,
 * 205, and 304 it produced.
 *
 * Lives here rather than beside its caller in `plugin-runtime.mjs` so a test
 * can reach it: that module awaits at its top level and speaks NDJSON over
 * stdio, so importing it starts a plugin runner.
 */
export function isNullBodyStatus(status) {
  return status === 101 || status === 103 || status === 204 || status === 205 || status === 304
}

import {
  isExactApplicationPath,
  normalizePresence,
  normalizeRealtime,
  RESERVED_FRAMEWORK_PATHS,
} from './plugin-registration.mjs'

/**
 * Re-exported, not redefined.
 *
 * The list and the rules around it live in
 * `packages/@ruvyxa/core/src/plugin-registration.ts` and are copied here by
 * `pnpm --filter ruvyxa sync:runtime`. This name stays exported from this module
 * because `serverless-handler.mjs` and the plugin tests already reach it here.
 */
export { RESERVED_FRAMEWORK_PATHS }

/**
 * Build the plugin registry by running every plugin's `register(api)`.
 *
 * Registration is the only place a plugin gets to declare anything, so the
 * validation here is the whole contract: duplicate names, malformed hooks,
 * conflicting routes, and reserved paths all fail loudly at construction
 * rather than on some later request.
 */
export async function createPluginRegistry({
  root,
  plugins: pluginsValue,
  markdown,
  environment,
} = {}) {
  const plugins = Array.isArray(pluginsValue) ? pluginsValue : []
  const names = new Set()
  const routeOwners = new Map()
  const registry = {
    root,
    markdown,
    // Production is the safe default: a host that did not state its
    // environment withholds development-only behaviour rather than enabling it
    // for a server that may be serving real traffic.
    environment: environment === 'development' ? 'development' : 'production',
    plugins: [],
    httpRequest: [],
    httpResponse: [],
    buildStart: [],
    buildResolve: [],
    buildLoad: [],
    buildTransform: [],
    buildComplete: [],
    devFileChange: [],
    diagnostics: [],
    capabilities: new Map(),
  }

  for (const [index, plugin] of plugins.entries()) {
    if (!plugin || typeof plugin !== 'object' || Array.isArray(plugin)) {
      throw new TypeError(`config.plugins[${index}] must be a plugin object`)
    }
    const name = typeof plugin.name === 'string' ? plugin.name.trim() : ''
    if (!name) throw new TypeError(`config.plugins[${index}] must have a non-empty name`)
    if (names.has(name)) throw new TypeError(`duplicate plugin name: ${name}`)
    if (typeof plugin.register !== 'function') {
      throw new TypeError(`plugin "${name}" must provide register(api)`)
    }
    names.add(name)
    registry.plugins.push(name)
    await plugin.register(createRegistrationApi(registry, name, routeOwners))
  }

  const errors = registry.diagnostics.filter((diagnostic) => diagnostic.level === 'error')
  if (errors.length > 0) {
    throw new TypeError(
      errors.map((diagnostic) => `${diagnostic.code} ${diagnostic.message}`).join('\n'),
    )
  }
  return registry
}

/** True when this registry has no HTTP behavior at all. */
export function hasPluginHttp(registry) {
  return Boolean(registry) && (registry.httpRequest.length > 0 || registry.httpResponse.length > 0)
}

/**
 * The request a hook is handed as context.
 *
 * A clone, so a hook that reads the body cannot take it from the route handler
 * that has to read it next. But a `Request` whose body has already been
 * consumed cannot be cloned at all — `clone()` throws `TypeError: unusable` —
 * and by the time the *response* hooks run, the route handler has usually
 * consumed it. That turned every request with a body into a 500 in any
 * deployment whose project registered an `http.onResponse` hook: the route ran,
 * produced its answer, and the response stage threw it away.
 *
 * A used body is gone either way, so the fallback hands over the same URL,
 * method, and headers with no body rather than failing. A hook that needs the
 * body reads it on the request side, where it is still there.
 */
function contextRequest(request) {
  if (!request.bodyUsed) return request.clone()
  return new Request(request.url, { method: request.method, headers: request.headers })
}

/**
 * Run the request-side hooks.
 *
 * Returns the short-circuit `Response` a hook produced, or the (possibly
 * rewritten) `Request` that should continue to routing.
 */
export async function dispatchPluginRequest(registry, initialRequest) {
  let request = initialRequest
  for (const entry of registry.httpRequest) {
    const pathname = decodedRequestPathname(request)
    if (entry.kind === 'route') {
      if (
        entry.path !== pathname ||
        (!entry.methods.includes('*') && !entry.methods.includes(request.method))
      ) {
        continue
      }
      const result = await entry.handler(
        Object.freeze({
          plugin: entry.plugin,
          root: registry.root,
          request: contextRequest(request),
        }),
      )
      if (!(result instanceof Response)) throw unsupportedReturn(entry.plugin, 'http.route')
      return { kind: 'response', response: result }
    }
    if (!matchesPatterns(entry.match, pathname)) continue

    let continued = request
    const context = Object.freeze({
      plugin: entry.plugin,
      root: registry.root,
      request: contextRequest(request),
      next(value = request) {
        if (!(value instanceof Request)) {
          throw new TypeError(`plugin "${entry.plugin}" http.onRequest().next() expects a Request`)
        }
        continued = value
      },
    })
    const result = await entry.handler(context)
    if (result instanceof Response) return { kind: 'response', response: result }
    if (result instanceof Request) request = result
    else if (result === undefined) request = continued
    else throw unsupportedReturn(entry.plugin, 'http.onRequest')
  }
  return { kind: 'request', request }
}

/** Run the response-side hooks and return the final `Response`. */
export async function dispatchPluginResponse(registry, request, initialResponse) {
  let response = initialResponse
  for (const entry of registry.httpResponse) {
    if (!matchesPatterns(entry.match, decodedRequestPathname(request))) continue
    let continued = response
    const context = Object.freeze({
      plugin: entry.plugin,
      root: registry.root,
      request: contextRequest(request),
      response: response.clone(),
      next(value = response) {
        if (!(value instanceof Response)) {
          throw new TypeError(
            `plugin "${entry.plugin}" http.onResponse().next() expects a Response`,
          )
        }
        continued = value
      },
    })
    const result = await entry.handler(context)
    if (result instanceof Response) response = result
    else if (result === undefined) response = continued
    else throw unsupportedReturn(entry.plugin, 'http.onResponse')
  }
  return response
}

/**
 * Diagnostics a registry's shape implies, added to the ones plugins reported.
 *
 * `build.onResolve` and `build.onLoad` are answered by the native bundler,
 * which builds the browser graph. The server and prerender graph is compiled by
 * `runtime/compiler.mjs`, which has no plugin host to ask — so a route that
 * *imports* a plugin-provided module renders with
 * `Cannot find package '<id>'` while its browser bundle is built correctly.
 * Nothing said so, and `examples/demo` registers such a hook without importing
 * what it provides, so the gap had never been exercised.
 */
function registryShapeDiagnostics(registry) {
  if (registry.buildResolve.length === 0 && registry.buildLoad.length === 0) return []
  return [
    {
      plugin: 'ruvyxa',
      code: 'RUV2107',
      // Informational, not a warning: registering these hooks is a supported
      // thing to do, and a project whose plugin-provided modules are imported
      // only from client components is entirely correct. `PluginDiagnosticLevel`
      // has no `warn` either — the three levels are info, warning, and error.
      level: 'info',
      message:
        'build.onResolve/onLoad apply to the browser graph only. A module they provide cannot be ' +
        'resolved while a page is server-rendered or pre-rendered, so import it from a client ' +
        'component, or write the file the resolve hook names.',
    },
  ]
}

/** Summary of what the registry declared, used by `ruvyxa` tooling output. */
/**
 * Run every `build.onTransform` hook over one module's source.
 *
 * Lives here, beside the HTTP dispatch, for the reason stated at the top of
 * this file: a registry that only one transport can reach is a feature only
 * that transport has. Build transforms were reachable from the NDJSON host
 * alone, which the Rust bundler speaks and the JavaScript compiler does not —
 * so a plugin rewrote the browser bundle while every server render read the
 * original file, and the two documents disagreed on any value that reached
 * markup. Both hosts now call this.
 *
 * `environment` is what the caller is compiling for, so a plugin can rewrite
 * one side deliberately (`if (environment !== 'client') return`) instead of
 * having that decided for it.
 *
 * Returns `{ code, map? }` when a hook changed the source, or `null`.
 */
export async function dispatchBuildTransform(registry, { code, id, environment }) {
  if (registry.buildTransform.length === 0) return null
  let current = String(code ?? '')
  let map
  let changed = false
  const base = buildHookContext(registry, environment)
  for (const entry of registry.buildTransform) {
    const context = Object.freeze({ ...base, code: current, id: String(id ?? '') })
    const result = normalizeCodeResult(entry.plugin, 'build.onTransform', await entry.hook(context))
    if (!result) continue
    current = result.code
    if (result.map !== undefined) map = result.map
    changed = true
  }
  return changed ? { code: current, ...(map === undefined ? {} : { map }) } : null
}

/** The `{ root, environment }` every build hook is given. */
export function buildHookContext(registry, environment) {
  const allowed = new Set(['client', 'server', 'edge', 'worker', 'shared'])
  return {
    root: registry.root,
    environment: allowed.has(environment) ? environment : 'client',
  }
}

/** Accept a hook's `string`, `{ code, map }`, or nothing; reject anything else. */
export function normalizeCodeResult(plugin, socket, result) {
  if (result === null || result === undefined) return null
  if (typeof result === 'string') return { code: result }
  if (result && typeof result === 'object' && typeof result.code === 'string') {
    return {
      code: result.code,
      ...(result.map === undefined || result.map === null
        ? {}
        : { map: typeof result.map === 'string' ? result.map : JSON.stringify(result.map) }),
    }
  }
  throw unsupportedReturn(plugin, socket)
}

export function describeRegistry(registry) {
  return {
    plugins: registry.plugins,
    // Top level, not under `http`: the environment is a property of the host,
    // not of its HTTP registrations.
    environment: registry.environment,
    http: {
      request: registry.httpRequest.length,
      response: registry.httpResponse.length,
      routes: registry.httpRequest.filter((entry) => entry.kind === 'route').length,
      requestMatch: patternUnion(registry.httpRequest),
      responseMatch: patternUnion(registry.httpResponse),
    },
    build: {
      start: registry.buildStart.length,
      resolve: registry.buildResolve.length,
      load: registry.buildLoad.length,
      transform: registry.buildTransform.length,
      complete: registry.buildComplete.length,
    },
    dev: { fileChange: registry.devFileChange.length },
    diagnostics: [...registry.diagnostics, ...registryShapeDiagnostics(registry)],
    capabilities: [...registry.capabilities.values()],
  }
}

export function unsupportedReturn(plugin, socket) {
  return new TypeError(`plugin "${plugin}" ${socket} returned an unsupported value`)
}

export function matchesPatterns(patterns, value) {
  if (!patterns || patterns.length === 0) return true
  return patterns.some((pattern) => {
    if (pattern === '*') return true
    if (pattern.endsWith('*')) return value.startsWith(pattern.slice(0, -1))
    return value === pattern
  })
}

/**
 * The path a hook is scoped against — the same one the router resolves.
 *
 * There must be exactly one answer to "what is this request's path". This used
 * to decode the whole pathname in one call, while `canonicalRoutePath` decodes
 * per segment and collapses the segment structure, so the two disagreed about
 * every request whose raw path was not already canonical. `//api/users` routed
 * to `/api/users` and read as *out of scope* for `['/api/*']` — the default
 * scope of `originGuard()` — so a plain cross-site
 * `<form method="POST" action="https://victim.example//api/users">` reached the
 * route handler with the session cookie attached and no guard ran. Every
 * path-scoped `http.onRequest`, `http.onResponse`, and `http.route` had the
 * same hole, and a scoped `securityHeaders` silently stopped applying.
 *
 * `route-match.mjs` is already carried in `HANDLER_RUNTIME_FILES` and already
 * copied beside this module in every function bundle, so sharing the router's
 * answer costs no new file anywhere.
 *
 * `canonicalRoutePath` returns `null` for a path the router refuses outright —
 * an encoded separator or a traversal component. Those never reach a plugin on
 * the native host, which answers 400 first, and the deployed host answers 400
 * in `dispatch`. Falling back to the raw pathname keeps the guard running for
 * the window in between: failing open there would hand back the bypass this
 * function exists to close.
 */
export function decodedRequestPathname(request) {
  const pathname = new URL(request.url).pathname
  return canonicalRoutePath(pathname) ?? pathname
}

function patternUnion(entries) {
  const patterns = new Set()
  for (const entry of entries) {
    if (entry.kind === 'route') {
      patterns.add(entry.path)
      continue
    }
    if (!entry.match || entry.match.length === 0 || entry.match.includes('*')) return null
    for (const pattern of entry.match) patterns.add(pattern)
  }
  return [...patterns]
}

// ─── Registration API ───────────────────────────────────────────────────────

function createRegistrationApi(registry, plugin, routeOwners) {
  return Object.freeze({
    // Registration-time rather than request-time on purpose: a plugin that
    // only makes sense in one environment declines to register at all, so the
    // other environment pays nothing — not even a per-request comparison.
    environment: registry.environment,
    http: Object.freeze({
      onRequest(value) {
        const registration = normalizeHttpHook(plugin, 'onRequest', value)
        registry.httpRequest.push({ plugin, kind: 'hook', ...registration })
      },
      onResponse(value) {
        registry.httpResponse.push({
          plugin,
          ...normalizeHttpHook(plugin, 'onResponse', value),
        })
      },
      route(value) {
        const route = normalizeHttpRoute(plugin, value)
        for (const method of route.methods) {
          const key = `${method} ${route.path}`
          const wildcardKey = `* ${route.path}`
          const conflict = routeOwners.get(key) ?? routeOwners.get(wildcardKey)
          if (conflict) {
            throw new TypeError(
              `plugin "${plugin}" route ${key} conflicts with plugin "${conflict}"`,
            )
          }
          if (method === '*') {
            const pathConflict = [...routeOwners.entries()].find(([candidate]) =>
              candidate.endsWith(` ${route.path}`),
            )
            if (pathConflict) {
              throw new TypeError(
                `plugin "${plugin}" route ${key} conflicts with plugin "${pathConflict[1]}"`,
              )
            }
          }
          routeOwners.set(key, plugin)
        }
        registry.httpRequest.push({ plugin, kind: 'route', ...route })
      },
    }),
    build: Object.freeze({
      onStart(hook) {
        registerHook(registry.buildStart, plugin, 'build.onStart', hook)
      },
      onResolve(hook) {
        registerHook(registry.buildResolve, plugin, 'build.onResolve', hook)
      },
      onLoad(hook) {
        registerHook(registry.buildLoad, plugin, 'build.onLoad', hook)
      },
      onTransform(hook) {
        registerHook(registry.buildTransform, plugin, 'build.onTransform', hook)
      },
      onComplete(hook) {
        registerHook(registry.buildComplete, plugin, 'build.onComplete', hook)
      },
    }),
    dev: Object.freeze({
      onFileChange(value) {
        const registration = normalizeDevFileChange(plugin, value)
        registry.devFileChange.push({ plugin, ...registration })
      },
    }),
    diagnostics: Object.freeze({
      report(value) {
        registry.diagnostics.push(normalizeDiagnostic(plugin, value))
      },
    }),
    native: Object.freeze({
      claim(capability, options = {}) {
        const normalize = nativeCapabilityNormalizer(capability)
        if (!normalize) {
          throw new TypeError(
            `plugin "${plugin}" requested unsupported native capability "${String(capability)}"`,
          )
        }
        const owner = registry.capabilities.get(capability)
        if (owner) {
          throw new TypeError(
            `plugin "${plugin}" cannot claim ${capability}; it is already owned by plugin "${owner.plugin}"`,
          )
        }
        registry.capabilities.set(capability, normalize(plugin, options))
      },
    }),
  })
}

function registerHook(collection, plugin, socket, hook) {
  if (typeof hook !== 'function') {
    throw new TypeError(`plugin "${plugin}" ${socket}() expects a function`)
  }
  collection.push({ plugin, hook })
}

function normalizeHttpHook(plugin, socket, value) {
  const registration = typeof value === 'function' ? { handler: value } : value
  if (!registration || typeof registration !== 'object' || Array.isArray(registration)) {
    throw new TypeError(`plugin "${plugin}" http.${socket}() expects a handler or options object`)
  }
  if (typeof registration.handler !== 'function') {
    throw new TypeError(`plugin "${plugin}" http.${socket}() requires handler`)
  }
  return {
    match: normalizePatterns(plugin, `http.${socket}().match`, registration.match),
    handler: registration.handler,
  }
}

/** Accept one method, a list, or nothing, and always answer with a list. */
function normalizeMethodList(method) {
  if (method === undefined) return ['*']
  return Array.isArray(method) ? method : [method]
}

function normalizeHttpRoute(plugin, value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`plugin "${plugin}" http.route() expects an options object`)
  }
  if (typeof value.path !== 'string' || !isExactApplicationPath(value.path)) {
    throw new TypeError(`plugin "${plugin}" http.route().path must be an exact absolute path`)
  }
  // The rule `RESERVED_FRAMEWORK_PATHS` exists to state, applied where it was
  // not: the constant was read only by the two socket normalisers, so a plugin
  // *route* at `/__ruvyxa/action` registered cleanly. Under `dev`/`start` the
  // framework answered and the route was dead; in a deployed build the route
  // answered and every server action 404'd at the plugin's discretion. Refusing
  // at registration is the one place both hosts can agree, and it is the same
  // refusal `normalizeRealtime` and `normalizePresence` already make.
  if (RESERVED_FRAMEWORK_PATHS.includes(value.path)) {
    throw new TypeError(
      `plugin "${plugin}" http.route() path "${value.path}" collides with a reserved framework route`,
    )
  }
  if (typeof value.handler !== 'function') {
    throw new TypeError(`plugin "${plugin}" http.route() requires handler`)
  }
  const input = normalizeMethodList(value.method)
  if (
    input.length === 0 ||
    input.some(
      (method) =>
        typeof method !== 'string' || !/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(method.trim()),
    )
  ) {
    throw new TypeError(
      `plugin "${plugin}" http.route().method must contain valid HTTP method tokens`,
    )
  }
  return {
    path: value.path,
    methods: [...new Set(input.map((method) => method.trim().toUpperCase()))],
    handler: value.handler,
  }
}

function normalizeDevFileChange(plugin, value) {
  const registration = typeof value === 'function' ? { handler: value } : value
  if (!registration || typeof registration !== 'object' || Array.isArray(registration)) {
    throw new TypeError(`plugin "${plugin}" dev.onFileChange() expects a handler or options object`)
  }
  if (typeof registration.handler !== 'function') {
    throw new TypeError(`plugin "${plugin}" dev.onFileChange() requires handler`)
  }
  return {
    match: normalizePatterns(plugin, 'dev.onFileChange().match', registration.match, false),
    handler: registration.handler,
  }
}

function normalizePatterns(plugin, field, value, requireSlash = true) {
  if (value === undefined) return undefined
  if (!Array.isArray(value) || value.length === 0) {
    throw new TypeError(`plugin "${plugin}" ${field} must contain at least one pattern`)
  }
  if (value.some((pattern) => typeof pattern !== 'string')) {
    throw new TypeError(`plugin "${plugin}" ${field} must be an array of strings`)
  }
  for (const [index, pattern] of value.entries()) {
    const wildcard = pattern.indexOf('*')
    const validStart = !requireSlash || pattern === '*' || pattern.startsWith('/')
    const validWildcard =
      wildcard === -1 || (wildcard === pattern.length - 1 && wildcard === pattern.lastIndexOf('*'))
    if (!pattern || !validStart || !validWildcard) {
      throw new TypeError(
        `plugin "${plugin}" ${field}[${index}] must ${requireSlash ? 'start with "/" and ' : ''}use a wildcard only at the end`,
      )
    }
  }
  return [...value]
}

function normalizeDiagnostic(plugin, value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`plugin "${plugin}" diagnostics.report() expects an object`)
  }
  if (!['info', 'warning', 'error'].includes(value.level)) {
    throw new TypeError(`plugin "${plugin}" diagnostic level must be info, warning, or error`)
  }
  if (typeof value.code !== 'string' || !/^[A-Z][A-Z0-9_-]{2,31}$/.test(value.code)) {
    throw new TypeError(`plugin "${plugin}" diagnostic code must be an uppercase identifier`)
  }
  if (typeof value.message !== 'string' || !value.message.trim()) {
    throw new TypeError(`plugin "${plugin}" diagnostic message must be non-empty`)
  }
  return Object.freeze({
    plugin,
    level: value.level,
    code: value.code,
    message: value.message.trim(),
  })
}

/**
 * Resolve the normalizer for a claimed capability, or `undefined` when the id
 * is not one this runtime serves.
 */
function nativeCapabilityNormalizer(capability) {
  if (capability === 'realtime@1') return normalizeRealtime
  if (capability === 'presence@1') return normalizePresence
  return undefined
}
