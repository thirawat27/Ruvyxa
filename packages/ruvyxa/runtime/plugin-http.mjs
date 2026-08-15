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

/**
 * Framework endpoints a plugin-declared route or native transport must not
 * claim.
 *
 * Held to `tests/fixtures/framework-endpoint-conformance.json` together with
 * `RESERVED_FRAMEWORK_ROUTES` in the native server, which panics inside axum
 * if a second handler registers one of these paths.
 */
export const RESERVED_FRAMEWORK_PATHS = Object.freeze([
  '/__ruvyxa/hmr',
  '/__ruvyxa/client',
  '/__ruvyxa/action',
  '/__ruvyxa/trace',
  '/__ruvyxa/devtools',
  '/__ruvyxa/devtools/data',
  '/__ruvyxa/image',
])

/**
 * Build the plugin registry by running every plugin's `register(api)`.
 *
 * Registration is the only place a plugin gets to declare anything, so the
 * validation here is the whole contract: duplicate names, malformed hooks,
 * conflicting routes, and reserved paths all fail loudly at construction
 * rather than on some later request.
 */
export async function createPluginRegistry({ root, plugins: pluginsValue, markdown } = {}) {
  const plugins = Array.isArray(pluginsValue) ? pluginsValue : []
  const names = new Set()
  const routeOwners = new Map()
  const registry = {
    root,
    markdown,
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
        Object.freeze({ plugin: entry.plugin, root: registry.root, request: request.clone() }),
      )
      if (!(result instanceof Response)) throw unsupportedReturn(entry.plugin, 'http.route')
      return { kind: 'response', response: result }
    }
    if (!matchesPatterns(entry.match, pathname)) continue

    let continued = request
    const context = Object.freeze({
      plugin: entry.plugin,
      root: registry.root,
      request: request.clone(),
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
      request: request.clone(),
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

/** Summary of what the registry declared, used by `ruvyxa` tooling output. */
export function describeRegistry(registry) {
  return {
    plugins: registry.plugins,
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
    diagnostics: registry.diagnostics,
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

/** Match paths using the decoded representation the Rust router exposes to plugins. */
export function decodedRequestPathname(request) {
  const pathname = new URL(request.url).pathname
  try {
    return decodeURIComponent(pathname)
  } catch {
    // A production host rejects malformed path encodings before this runtime
    // receives them. Preserve the encoded value defensively for direct calls.
    return pathname
  }
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

function normalizeHttpRoute(plugin, value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`plugin "${plugin}" http.route() expects an options object`)
  }
  if (typeof value.path !== 'string' || !isExactApplicationPath(value.path)) {
    throw new TypeError(`plugin "${plugin}" http.route().path must be an exact absolute path`)
  }
  if (typeof value.handler !== 'function') {
    throw new TypeError(`plugin "${plugin}" http.route() requires handler`)
  }
  const input =
    value.method === undefined ? ['*'] : Array.isArray(value.method) ? value.method : [value.method]
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

function isExactApplicationPath(value) {
  return (
    value.startsWith('/') && !value.includes('?') && !value.includes('#') && !value.includes('*')
  )
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

function normalizeRealtime(plugin, value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`plugin "${plugin}" native.claim('realtime@1') expects an options object`)
  }
  const pathValue = value.path ?? '/__ruvyxa/realtime'
  const heartbeatMs = value.heartbeatMs ?? 25_000
  const capacity = value.capacity ?? 256
  if (!isExactApplicationPath(pathValue)) {
    throw new TypeError(`plugin "${plugin}" realtime path must be an exact absolute path`)
  }
  if (!Number.isInteger(heartbeatMs) || heartbeatMs < 5_000 || heartbeatMs > 120_000) {
    throw new TypeError(`plugin "${plugin}" realtime heartbeatMs must be between 5000 and 120000`)
  }
  if (!Number.isInteger(capacity) || capacity < 16 || capacity > 4096) {
    throw new TypeError(`plugin "${plugin}" realtime capacity must be between 16 and 4096`)
  }
  if (RESERVED_FRAMEWORK_PATHS.includes(pathValue)) {
    throw new TypeError(
      `plugin "${plugin}" realtime path "${pathValue}" collides with a reserved framework route`,
    )
  }
  return Object.freeze({ id: 'realtime@1', plugin, path: pathValue, heartbeatMs, capacity })
}

function normalizePresence(plugin, value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`plugin "${plugin}" native.claim('presence@1') expects an options object`)
  }
  const pathValue = value.path ?? '/__ruvyxa/collab'
  const heartbeatMs = value.heartbeatMs ?? 25_000
  if (!isExactApplicationPath(pathValue)) {
    throw new TypeError(`plugin "${plugin}" presence path must be an exact absolute path`)
  }
  if (!Number.isInteger(heartbeatMs) || heartbeatMs < 5_000 || heartbeatMs > 120_000) {
    throw new TypeError(`plugin "${plugin}" presence heartbeatMs must be between 5000 and 120000`)
  }
  if (RESERVED_FRAMEWORK_PATHS.includes(pathValue)) {
    throw new TypeError(
      `plugin "${plugin}" presence path "${pathValue}" collides with a reserved framework route`,
    )
  }
  return Object.freeze({ id: 'presence@1', plugin, path: pathValue, heartbeatMs })
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
