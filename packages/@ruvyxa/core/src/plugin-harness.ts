import { canonicalRoutePath } from './route-match.js'
import { isExactApplicationPath, isReservedFrameworkPath } from './plugin-registration.js'
import type {
  PluginBuildContext,
  PluginBuildLoadHandler,
  PluginBuildResolveHandler,
  PluginBuildStartHook,
  PluginBuildTransformHandler,
  PluginDevFileChangeHandler,
  PluginDiagnostic,
  PluginEnvironment,
  PluginHeadEntry,
  PluginHostEnvironment,
  PluginHttpRequestHandler,
  PluginHttpResponseHandler,
  PluginHttpRouteContext,
  PluginHttpRouteRegistration,
  PluginNativeCapability,
  PluginRegistrationApi,
  PluginRoutePattern,
  PresencePluginOptions,
  RealtimePluginOptions,
  RuvyxaPlugin,
  TransformResult,
} from './types.js'

/**
 * Test harness for Ruvyxa plugins.
 *
 * A plugin's behavior lives inside `register(api)`, which the framework calls
 * once at startup. Without a harness the only way to assert on a header, a
 * redirect, or a transform is to boot a real server and drive HTTP at it, which
 * is slow enough that most plugins end up untested.
 *
 * `createPluginHarness` runs `register` against recording sockets and exposes
 * the same entry points the server does, so a plugin can be exercised as a
 * plain unit:
 *
 * ```ts
 * const harness = await createPluginHarness(headers([{ source: '/api/*', headers: { 'x-a': '1' } }]))
 * const response = await harness.respond(new Response('ok'), { path: '/api/items' })
 * assert.equal(response.headers.get('x-a'), '1')
 * ```
 *
 * Route-pattern semantics match the server's: `*` matches everything, a
 * trailing `*` matches by prefix, anything else matches exactly.
 *
 * ## Why registration validates
 *
 * The harness used to record every registration into an array and check none of
 * it, so a plugin whose `match` pattern, route path, method token, diagnostic
 * level, or capability claim `createPluginRegistry` refuses at construction had
 * a green suite and a `ruvyxa dev` that would not start — the one failure the
 * harness exists to prevent. The refusals below mirror
 * `runtime/plugin-http.mjs` rule for rule; the dispatch order and the decoded
 * pathname mirror `dispatchPluginRequest`.
 *
 * The rules are duplicated here rather than shared because the dependency runs
 * the wrong way: `ruvyxa` depends on `@ruvyxa/core`, so the harness cannot
 * import the runtime's copy. The end state is the normalisers moving *into*
 * this package and `plugin-http.mjs` importing them through the generated-copy
 * mechanism in `packages/ruvyxa/scripts/sync-shared-runtime.mjs`, which is
 * `--check`-gated. Until that lands, any change to one copy belongs in both.
 */

/** A native capability a plugin claimed, in claim order. */
export interface HarnessNativeClaim {
  plugin: string
  capability: PluginNativeCapability
  options: RealtimePluginOptions | PresencePluginOptions
}

/** A diagnostic a plugin reported during registration. */
export interface HarnessDiagnostic extends PluginDiagnostic {
  plugin: string
}

/** A file-change notification delivered to `dev.onFileChange`. */
export interface HarnessFileChange {
  /** Changed paths, relative to the application root. */
  paths: readonly string[]
}

export interface HarnessRequestOptions {
  /** Application path used for pattern matching. Derived from the URL otherwise. */
  path?: string
  method?: string
  headers?: HeadersInit
  body?: BodyInit | null
}

export interface HarnessBuildOptions {
  root?: string
  outDir?: string
  manifest?: Record<string, unknown>
  environment?: PluginEnvironment
}

export interface PluginHarness {
  /** Head elements the plugins declared, in configuration order. */
  readonly head: readonly PluginHeadEntry[]
  /** Routes the plugins registered, in registration order. */
  readonly routes: readonly PluginHttpRouteRegistration[]
  /** Diagnostics reported during registration. */
  readonly diagnostics: readonly HarnessDiagnostic[]
  /** Native capabilities claimed during registration. */
  readonly nativeClaims: readonly HarnessNativeClaim[]

  /**
   * Run the request pipeline for one incoming request.
   *
   * Routes and `onRequest` hooks share one registration-ordered list, as they do
   * in the server, so a route registered before a hook answers first.
   *
   * Returns the `Response` a plugin short-circuited with — from a hook or from a
   * matching route — or the (possibly replaced) `Request` that would have
   * continued to the router.
   */
  request(
    input: Request | string,
    options?: HarnessRequestOptions,
  ): Promise<{ response?: Response; request: Request }>

  /** Run the response hooks over one outgoing response. */
  respond(
    response: Response,
    input?: Request | string,
    options?: HarnessRequestOptions,
  ): Promise<Response>

  /** Invoke a registered route handler, or `undefined` when none matches. */
  route(input: Request | string, options?: HarnessRequestOptions): Promise<Response | undefined>

  /**
   * Deliver a file-change notification to matching `dev.onFileChange` hooks.
   *
   * A registration's `match` filters the delivered paths; a hook runs when at
   * least one changed path matches, and receives only the matching ones.
   */
  fileChange(change: HarnessFileChange | string | readonly string[]): Promise<void>

  /** Build hooks, in the order the build calls them. */
  readonly build: {
    start(options?: HarnessBuildOptions): Promise<void>
    resolve(id: string, importer?: string, options?: HarnessBuildOptions): Promise<string | null>
    load(id: string, options?: HarnessBuildOptions): Promise<TransformResult | null>
    transform(
      code: string,
      id: string,
      options?: HarnessBuildOptions,
    ): Promise<TransformResult | null>
    complete(options?: HarnessBuildOptions): Promise<void>
  }
}

const DEFAULT_ROOT = '/project'
const DEFAULT_ORIGIN = 'http://localhost'

/**
 * One entry of the registration-ordered request pipeline.
 *
 * Routes and `onRequest` hooks live in a single list because the server
 * dispatches them from one: a route registered before a hook answers before the
 * hook ever runs. Two arrays reached by two methods could not express that, so
 * a plugin whose behaviour depends on the order had no way to be tested.
 */
type HarnessRequestEntry =
  | {
      kind: 'hook'
      plugin: string
      match?: readonly PluginRoutePattern[]
      handler: PluginHttpRequestHandler
    }
  | {
      kind: 'route'
      plugin: string
      path: string
      methods: readonly string[]
      handler: (context: PluginHttpRouteContext) => Response | Promise<Response>
    }

interface HarnessResponseEntry {
  plugin: string
  match?: readonly PluginRoutePattern[]
  handler: PluginHttpResponseHandler
}

interface HarnessFileChangeEntry {
  plugin: string
  match?: readonly string[]
  handler: PluginDevFileChangeHandler
}

const NATIVE_CAPABILITIES = new Set<string>(['realtime@1', 'presence@1'])

/**
 * Register one or more plugins against recording sockets.
 *
 * Plugins run in the order given, which is the order `config({ plugins })`
 * applies them.
 */
export async function createPluginHarness(
  plugins: RuvyxaPlugin | readonly RuvyxaPlugin[],
  options: { root?: string; environment?: PluginHostEnvironment } = {},
): Promise<PluginHarness> {
  const list = Array.isArray(plugins) ? [...plugins] : [plugins as RuvyxaPlugin]
  const root = options.root ?? DEFAULT_ROOT
  // Matches what a host reports when it says nothing, so a plugin under test
  // behaves here the way it does in a deployment by default. Pass
  // `environment: 'development'` to exercise the other branch.
  const environment = options.environment ?? 'production'

  const httpRequest: HarnessRequestEntry[] = []
  const responseHooks: HarnessResponseEntry[] = []
  const routes: PluginHttpRouteRegistration[] = []
  const routeOwners = new Map<string, string>()
  const fileChangeHooks: HarnessFileChangeEntry[] = []
  const buildHooks = {
    start: [] as PluginBuildStartHook[],
    resolve: [] as PluginBuildResolveHandler[],
    load: [] as PluginBuildLoadHandler[],
    transform: [] as PluginBuildTransformHandler[],
    complete: [] as Array<(context: PluginBuildContext) => void | Promise<void>>,
  }
  const diagnostics: HarnessDiagnostic[] = []
  const nativeClaims: HarnessNativeClaim[] = []
  const head: PluginHeadEntry[] = []

  const names = new Set<string>()
  const capabilityOwners = new Map<PluginNativeCapability, string>()

  for (const [index, plugin] of list.entries()) {
    if (!plugin || typeof plugin !== 'object' || Array.isArray(plugin)) {
      throw new TypeError(`plugins[${index}] must be a plugin object`)
    }
    const name = typeof plugin.name === 'string' ? plugin.name.trim() : ''
    if (!name) throw new TypeError(`plugins[${index}] must have a non-empty name`)
    if (names.has(name)) throw new TypeError(`duplicate plugin name: ${name}`)
    if (typeof plugin.register !== 'function') {
      throw new TypeError(`plugin "${name}" must provide register(api)`)
    }
    names.add(name)

    for (const entry of plugin.head ?? []) head.push(entry)

    const api: PluginRegistrationApi = {
      environment,
      http: {
        onRequest(registration) {
          const normalized = normalizeHttpHook(name, 'onRequest', registration)
          httpRequest.push({ kind: 'hook', plugin: name, ...normalized })
        },
        onResponse(registration) {
          responseHooks.push({
            plugin: name,
            ...normalizeHttpHook<PluginHttpResponseHandler>(name, 'onResponse', registration),
          })
        },
        route(registration) {
          const route = normalizeHttpRoute(name, registration)
          claimRoute(routeOwners, name, route)
          routes.push(registration)
          httpRequest.push({ kind: 'route', plugin: name, ...route })
        },
      },
      build: {
        onStart: (hook) => {
          buildHooks.start.push(hook)
        },
        onResolve: (hook) => {
          buildHooks.resolve.push(hook)
        },
        onLoad: (hook) => {
          buildHooks.load.push(hook)
        },
        onTransform: (hook) => {
          buildHooks.transform.push(hook)
        },
        onComplete: (hook) => {
          buildHooks.complete.push(hook)
        },
      },
      dev: {
        onFileChange(registration) {
          fileChangeHooks.push({
            plugin: name,
            ...normalizeDevFileChange(name, registration),
          })
        },
      },
      diagnostics: {
        report(diagnostic) {
          diagnostics.push(normalizeDiagnostic(name, diagnostic))
        },
      },
      native: {
        claim(capability, claimOptions) {
          if (!NATIVE_CAPABILITIES.has(capability)) {
            throw new TypeError(
              `plugin "${name}" requested unsupported native capability "${String(capability)}"`,
            )
          }
          const owner = capabilityOwners.get(capability)
          if (owner) {
            throw new TypeError(
              `plugin "${name}" cannot claim ${capability}; it is already owned by plugin "${owner}"`,
            )
          }
          capabilityOwners.set(capability, name)
          nativeClaims.push({ plugin: name, capability, options: claimOptions ?? {} })
        },
      },
    }

    await plugin.register(api)
  }

  // The registry rejects the whole configuration when any plugin reported an
  // error, so a harness that merely collected them said a plugin was fine that
  // the framework refuses to boot with.
  const errors = diagnostics.filter((diagnostic) => diagnostic.level === 'error')
  if (errors.length > 0) {
    throw new TypeError(
      errors.map((diagnostic) => `${diagnostic.code} ${diagnostic.message}`).join('\n'),
    )
  }

  const buildContext = (options: HarnessBuildOptions = {}): PluginBuildContext => ({
    root: options.root ?? root,
    outDir: options.outDir ?? `${options.root ?? root}/.ruvyxa`,
    manifest: options.manifest ?? {},
  })
  const transformContext = (options: HarnessBuildOptions = {}) => ({
    root: options.root ?? root,
    environment: options.environment ?? ('server' as PluginEnvironment),
  })

  return {
    head,
    routes,
    diagnostics,
    nativeClaims,

    async request(input, requestOptions = {}) {
      let request = toRequest(input, requestOptions)
      for (const entry of httpRequest) {
        // Recomputed per entry: a hook may have replaced the request.
        const path = requestOptions.path ?? decodedRequestPathname(request)
        if (entry.kind === 'route') {
          if (entry.path !== path || !methodMatches(entry.methods, request.method)) continue
          const result = await entry.handler({ plugin: entry.plugin, root, request })
          if (!(result instanceof Response)) throw unsupportedReturn(entry.plugin, 'http.route')
          return { response: result, request }
        }
        if (!matchesPatterns(entry.match, path)) continue

        let continued = request
        const current = request
        const result = await entry.handler({
          plugin: entry.plugin,
          root,
          request,
          // `next()` is how a hook says "keep going"; a returned value wins.
          next(replacement = current) {
            if (!(replacement instanceof Request)) {
              throw new TypeError(
                `plugin "${entry.plugin}" http.onRequest().next() expects a Request`,
              )
            }
            continued = replacement
          },
        })
        if (result instanceof Response) return { response: result, request }
        if (result instanceof Request) request = result
        else if (result === undefined) request = continued
        else throw unsupportedReturn(entry.plugin, 'http.onRequest')
      }
      return { request }
    },

    async respond(response, input = '/', requestOptions = {}) {
      const request = toRequest(input, requestOptions)
      const path = requestOptions.path ?? decodedRequestPathname(request)
      let current = response
      for (const entry of responseHooks) {
        if (!matchesPatterns(entry.match, path)) continue
        let continued = current
        const incoming = current
        const result = await entry.handler({
          plugin: entry.plugin,
          root,
          request,
          response: current,
          next(replacement = incoming) {
            if (!(replacement instanceof Response)) {
              throw new TypeError(
                `plugin "${entry.plugin}" http.onResponse().next() expects a Response`,
              )
            }
            continued = replacement
          },
        })
        if (result instanceof Response) current = result
        else if (result === undefined) current = continued
        else throw unsupportedReturn(entry.plugin, 'http.onResponse')
      }
      return current
    },

    async route(input, requestOptions = {}) {
      const request = toRequest(input, requestOptions)
      const path = requestOptions.path ?? decodedRequestPathname(request)
      for (const entry of httpRequest) {
        if (entry.kind !== 'route') continue
        if (entry.path !== path || !methodMatches(entry.methods, request.method)) continue
        const result = await entry.handler({ plugin: entry.plugin, root, request })
        if (!(result instanceof Response)) throw unsupportedReturn(entry.plugin, 'http.route')
        return result
      }
      return undefined
    },

    async fileChange(change) {
      const paths = normalizeChangedPaths(change)
      for (const entry of fileChangeHooks) {
        const matched = paths.filter((path) => matchesPatterns(entry.match, path))
        if (matched.length === 0) continue
        await entry.handler({ root, paths: matched })
      }
    },

    build: {
      async start(options) {
        const context = buildContext(options)
        for (const hook of buildHooks.start) {
          await hook({ root: context.root, outDir: context.outDir })
        }
      },
      async resolve(id, importer, options) {
        for (const hook of buildHooks.resolve) {
          const result = await hook({ ...transformContext(options), id, importer })
          if (typeof result === 'string') return result
        }
        return null
      },
      async load(id, options) {
        for (const hook of buildHooks.load) {
          const result = await hook({ ...transformContext(options), id })
          const normalized = asTransformResult(result)
          if (normalized) return normalized
        }
        return null
      },
      async transform(code, id, options) {
        let current: TransformResult | null = null
        for (const hook of buildHooks.transform) {
          const result = await hook({
            ...transformContext(options),
            code: current?.code ?? code,
            id,
          })
          const normalized = asTransformResult(result)
          if (normalized) current = normalized
        }
        return current
      },
      async complete(options) {
        const context = buildContext(options)
        for (const hook of buildHooks.complete) await hook(context)
      },
    },
  }
}

/**
 * Accept a bare handler or an options object, and refuse anything else.
 *
 * Mirrors `normalizeHttpHook` in `runtime/plugin-http.mjs`.
 */
function normalizeHttpHook<THandler>(
  plugin: string,
  socket: 'onRequest' | 'onResponse',
  value: { match?: readonly PluginRoutePattern[]; handler: THandler } | THandler,
): { match?: readonly PluginRoutePattern[]; handler: THandler } {
  const registration =
    typeof value === 'function'
      ? { handler: value as THandler }
      : (value as { match?: readonly PluginRoutePattern[]; handler: THandler })
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
function normalizeMethodList(method: PluginHttpRouteRegistration['method']): readonly unknown[] {
  if (method === undefined) return ['*']
  return Array.isArray(method) ? method : [method]
}

/** Mirrors `normalizeHttpRoute` in `runtime/plugin-http.mjs`. */
function normalizeHttpRoute(
  plugin: string,
  value: PluginHttpRouteRegistration,
): {
  path: string
  methods: readonly string[]
  handler: (context: PluginHttpRouteContext) => Response | Promise<Response>
} {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`plugin "${plugin}" http.route() expects an options object`)
  }
  if (typeof value.path !== 'string' || !isExactApplicationPath(value.path)) {
    throw new TypeError(`plugin "${plugin}" http.route().path must be an exact absolute path`)
  }
  // The harness accepted this and the runtime refuses it, so a plugin could
  // pass the tests that validate it and be rejected by the server that runs it.
  // `plugin-http.mjs` has always made this refusal; the two are one rule now.
  if (isReservedFrameworkPath(value.path)) {
    throw new TypeError(
      `plugin "${plugin}" http.route().path "${value.path}" collides with a reserved framework route`,
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
    methods: [
      ...new Set((input as readonly string[]).map((method) => method.trim().toUpperCase())),
    ],
    handler: value.handler,
  }
}

/**
 * Refuse a path a previously registered route already answers.
 *
 * Mirrors the conflict check in `createRegistrationApi`: two plugins claiming
 * one route is a startup `TypeError`, not a first-registration-wins race.
 */
function claimRoute(
  routeOwners: Map<string, string>,
  plugin: string,
  route: { path: string; methods: readonly string[] },
): void {
  for (const method of route.methods) {
    const key = `${method} ${route.path}`
    const conflict = routeOwners.get(key) ?? routeOwners.get(`* ${route.path}`)
    if (conflict) {
      throw new TypeError(`plugin "${plugin}" route ${key} conflicts with plugin "${conflict}"`)
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
}

/** Mirrors `normalizeDevFileChange` in `runtime/plugin-http.mjs`. */
function normalizeDevFileChange(
  plugin: string,
  value:
    { match?: readonly string[]; handler: PluginDevFileChangeHandler } | PluginDevFileChangeHandler,
): { match?: readonly string[]; handler: PluginDevFileChangeHandler } {
  const registration =
    typeof value === 'function'
      ? { handler: value }
      : (value as { match?: readonly string[]; handler: PluginDevFileChangeHandler })
  if (!registration || typeof registration !== 'object' || Array.isArray(registration)) {
    throw new TypeError(`plugin "${plugin}" dev.onFileChange() expects a handler or options object`)
  }
  if (typeof registration.handler !== 'function') {
    throw new TypeError(`plugin "${plugin}" dev.onFileChange() requires handler`)
  }
  return {
    // Watched paths are application-relative, so they carry no leading slash.
    match: normalizePatterns(plugin, 'dev.onFileChange().match', registration.match, false),
    handler: registration.handler,
  }
}

/** Mirrors `normalizePatterns` in `runtime/plugin-http.mjs`. */
function normalizePatterns(
  plugin: string,
  field: string,
  value: readonly string[] | undefined,
  requireSlash = true,
): readonly string[] | undefined {
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

/** Mirrors `normalizeDiagnostic` in `runtime/plugin-http.mjs`. */
function normalizeDiagnostic(plugin: string, value: PluginDiagnostic): HarnessDiagnostic {
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
  return { plugin, level: value.level, code: value.code, message: value.message.trim() }
}

function unsupportedReturn(plugin: string, socket: string): TypeError {
  return new TypeError(`plugin "${plugin}" ${socket} returned an unsupported value`)
}

function asTransformResult(result: string | TransformResult | null | void): TransformResult | null {
  if (typeof result === 'string') return { code: result }
  if (result && typeof result === 'object' && typeof result.code === 'string') return result
  return null
}

function toRequest(input: Request | string, options: HarnessRequestOptions): Request {
  if (input instanceof Request) return input
  const url = input.startsWith('http') ? input : `${DEFAULT_ORIGIN}${input}`
  return new Request(url, {
    method: options.method ?? 'GET',
    headers: options.headers,
    body: options.body ?? null,
  })
}

function normalizeChangedPaths(change: HarnessFileChange | string | readonly string[]): string[] {
  if (typeof change === 'string') return [change]
  if (Array.isArray(change)) return [...(change as readonly string[])]
  return [...(change as HarnessFileChange).paths]
}

/**
 * The path a hook is scoped against — the same one the server resolves.
 *
 * The server matches the *decoded* pathname, through the router's own
 * `canonicalRoutePath`, so `/files/my%20doc` is in scope for a hook declared as
 * `['/files/my doc']` and `//api/users` is in scope for `['/api/*']`. Matching
 * the raw pathname here reported "not in scope" for requests the server hands
 * straight to the hook, which is how a scoped `originGuard()` could look
 * correctly configured and never run.
 *
 * `canonicalRoutePath` answers `null` for a path the router refuses outright;
 * falling back to the raw pathname keeps the hook running rather than failing
 * open, exactly as `decodedRequestPathname` does in `plugin-http.mjs`.
 */
function decodedRequestPathname(request: Request): string {
  const pathname = new URL(request.url).pathname
  return canonicalRoutePath(pathname) ?? pathname
}

function methodMatches(declared: readonly string[], method: string): boolean {
  return declared.includes('*') || declared.includes(method)
}

/**
 * Server route-pattern semantics: `*` matches everything, a trailing `*`
 * matches by prefix, anything else matches exactly. An absent list matches
 * every path.
 *
 * Mirrors `matchesPatterns` in `runtime/plugin-http.mjs`.
 */
function matchesPatterns(
  patterns: readonly PluginRoutePattern[] | undefined,
  path: string,
): boolean {
  if (!patterns || patterns.length === 0) return true
  return patterns.some((pattern) => {
    if (pattern === '*') return true
    if (pattern.endsWith('*')) return path.startsWith(pattern.slice(0, -1))
    return pattern === path
  })
}
