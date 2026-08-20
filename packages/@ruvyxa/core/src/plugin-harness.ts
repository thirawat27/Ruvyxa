import type {
  PluginBuildContext,
  PluginBuildLoadHandler,
  PluginBuildResolveHandler,
  PluginBuildStartHook,
  PluginBuildTransformHandler,
  PluginDevFileChangeRegistration,
  PluginDiagnostic,
  PluginEnvironment,
  PluginHeadEntry,
  PluginHostEnvironment,
  PluginHttpRequestRegistration,
  PluginHttpResponseRegistration,
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
   * Run the request hooks for one incoming request.
   *
   * Returns the `Response` a plugin short-circuited with, or the (possibly
   * replaced) `Request` that would have continued to the router.
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

  const requestHooks: Array<{ plugin: string; registration: PluginHttpRequestRegistration }> = []
  const responseHooks: Array<{ plugin: string; registration: PluginHttpResponseRegistration }> = []
  const routes: Array<{ plugin: string; registration: PluginHttpRouteRegistration }> = []
  const fileChangeHooks: Array<{ plugin: string; registration: PluginDevFileChangeRegistration }> =
    []
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

  for (const plugin of list) {
    for (const entry of plugin.head ?? []) head.push(entry)

    const api: PluginRegistrationApi = {
      environment,
      http: {
        onRequest(registration) {
          requestHooks.push({ plugin: plugin.name, registration: asRegistration(registration) })
        },
        onResponse(registration) {
          responseHooks.push({ plugin: plugin.name, registration: asRegistration(registration) })
        },
        route(registration) {
          routes.push({ plugin: plugin.name, registration })
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
            plugin: plugin.name,
            registration:
              typeof registration === 'function' ? { handler: registration } : registration,
          })
        },
      },
      diagnostics: {
        report(diagnostic) {
          diagnostics.push({ ...diagnostic, plugin: plugin.name })
        },
      },
      native: {
        claim(capability, claimOptions) {
          nativeClaims.push({ plugin: plugin.name, capability, options: claimOptions ?? {} })
        },
      },
    }

    await plugin.register(api)
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
    routes: routes.map((entry) => entry.registration),
    diagnostics,
    nativeClaims,

    async request(input, requestOptions = {}) {
      let request = toRequest(input, requestOptions)
      const path = requestOptions.path ?? pathOf(request)
      for (const { plugin, registration } of requestHooks) {
        if (!matchesAny(registration.match, path)) continue
        const result = await registration.handler({
          plugin,
          root,
          request,
          // `next()` is how a hook says "keep going"; a returned value wins.
          next(replacement) {
            if (replacement) request = replacement
          },
        })
        if (result instanceof Response) return { response: result, request }
        if (result instanceof Request) request = result
      }
      return { request }
    },

    async respond(response, input = '/', requestOptions = {}) {
      const request = toRequest(input, requestOptions)
      const path = requestOptions.path ?? pathOf(request)
      let current = response
      for (const { plugin, registration } of responseHooks) {
        if (!matchesAny(registration.match, path)) continue
        const result = await registration.handler({
          plugin,
          root,
          request,
          response: current,
          next(replacement) {
            if (replacement) current = replacement
          },
        })
        if (result instanceof Response) current = result
      }
      return current
    },

    async route(input, requestOptions = {}) {
      const request = toRequest(input, requestOptions)
      const path = requestOptions.path ?? pathOf(request)
      const method = request.method.toUpperCase()
      for (const { plugin, registration } of routes) {
        if (registration.path !== path) continue
        if (!methodMatches(registration.method, method)) continue
        return registration.handler({ plugin, root, request })
      }
      return undefined
    },

    async fileChange(change) {
      const paths = normalizeChangedPaths(change)
      for (const { registration } of fileChangeHooks) {
        const matched = paths.filter((path) => matchesAny(registration.match, path))
        if (matched.length === 0) continue
        await registration.handler({ root, paths: matched })
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

function asRegistration<THandler>(
  registration: { match?: readonly PluginRoutePattern[]; handler: THandler } | THandler,
): { match?: readonly PluginRoutePattern[]; handler: THandler } {
  return typeof registration === 'function'
    ? { handler: registration as THandler }
    : (registration as { match?: readonly PluginRoutePattern[]; handler: THandler })
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

function pathOf(request: Request): string {
  return new URL(request.url).pathname
}

function methodMatches(declared: PluginHttpRouteRegistration['method'], method: string): boolean {
  if (declared === undefined) return true
  const allowed = typeof declared === 'string' ? [declared] : declared
  return allowed.some((value) => value.toUpperCase() === method)
}

/**
 * Server route-pattern semantics: `*` matches everything, a trailing `*`
 * matches by prefix, anything else matches exactly. An absent list matches
 * every path.
 */
function matchesAny(patterns: readonly PluginRoutePattern[] | undefined, path: string): boolean {
  if (!patterns || patterns.length === 0) return true
  return patterns.some((pattern) => {
    if (pattern === '*') return true
    if (pattern.endsWith('*')) return path.startsWith(pattern.slice(0, -1))
    return pattern === path
  })
}
