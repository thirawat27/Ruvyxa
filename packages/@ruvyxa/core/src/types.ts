export interface RuvyxaConfig {
  appDir?: string
  outDir?: string
  /** Runtime used for config, SSR, rendering, and JavaScript plugins. @default 'node' */
  runtime?: 'node' | 'bun' | 'edge' | 'static'
  react?: boolean
  typescript?: {
    strict?: boolean
  }
  css?: {
    /** Additional project-relative global stylesheet files or directories. */
    entries?: string[]
  }
  server?: {
    port?: number
    host?: string
  }
  build?: {
    minify?: boolean
    map?: boolean
    treeShake?: boolean
    split?: 'single' | 'route' | 'manual'
    workers?: number
    jsx?: 'classic' | 'automatic'
    target?: 'es2018' | 'es2019' | 'es2020' | 'es2022' | 'esnext'
    manifest?: boolean
    /** Precompile dev route modules and load their dependencies in background workers. */
    warm?: boolean
    /** Reuse fingerprint-validated final prerender HTML between production builds. @default true */
    prerenderCache?: boolean
  }
  render?: RenderConfig
  debug?: {
    overlay?: boolean
    traces?: boolean
  }
  image?: ImageConfig
  security?: {
    /** Maximum server-action payload size in bytes. @default 1048576 */
    actionLimit?: number
    /** Maximum API route request payload size in bytes. @default 10485760 */
    apiLimit?: number
    /**
     * Maximum response size buffered by a response-phase Wasm plugin in bytes.
     * @default 33554432
     * @maximum 268435456
     */
    pluginLimit?: number
    /** Per-client/action request ceiling; values are bounded but configurable. */
    actionRateLimit?: {
      /** Maximum requests during `window` seconds. @default 600 */
      max?: number
      /** Rolling rate-limit window in seconds. @default 60 */
      window?: number
    }
    sameOrigin?: boolean
    fetchMeta?: boolean
    /**
     * Exact non-loopback reverse-proxy IPs allowed to supply X-Forwarded-For,
     * X-Real-IP, and X-Forwarded-Proto. Loopback proxies are trusted by default.
     */
    trustedProxyIps?: string[]
    headers?: boolean
  }
  cache?: {
    routes?: boolean
    css?: boolean
    /** Shared compile-cache directory. Relative paths are resolved from the project root. */
    dir?: string
  }
  middleware?: MiddlewareConfig
  adapter?: Adapter
  adapterOptions?: Record<string, unknown>
  plugins?: RuvyxaPlugin[]
}

export interface ImageConfig {
  /** Convert local PNG/JPEG public assets to WebP during production builds. @default true */
  optimize?: boolean
  /** Lossy WebP quality from 1 to 100. @default 82 */
  quality?: number
  /** Use lossless WebP encoding; `quality` then controls encoder effort. @default false */
  lossless?: boolean
  /** Image conversion workers. Zero selects the available CPU count. @default 0 */
  workers?: number
}

// ─── Rendering Strategy ───────────────────────────────────────────────────────

/**
 * Rendering strategy for a route. Determines when and how HTML is generated.
 *
 * - `"ssr"` — Server-Side Rendering: HTML generated on every request (default).
 * - `"ssg"` — Static Site Generation: HTML pre-rendered at build time.
 * - `"isr"` — Incremental Static Regeneration: pre-rendered at build, revalidated after TTL.
 * - `"csr"` — Client-Side Rendering: minimal shell served, full render in browser.
 * - `"ppr"` — Partial Pre-Rendering: static shell at build time + dynamic streaming at request time.
 */
export type RenderStrategy = 'ssr' | 'ssg' | 'isr' | 'csr' | 'ppr'

/**
 * Global render configuration in `ruvyxa.config.ts`.
 */
export interface RenderConfig {
  /**
   * Default rendering strategy for pages that don't declare one explicitly.
   * @default "ssr"
   */
  strategy?: RenderStrategy
  /**
   * Default ISR revalidation interval in seconds (used when a page exports
   * `revalidate` without a value or inherits ISR from config).
   * @default 60
   */
  revalidate?: number
}

// ─── Per-Page Exports ─────────────────────────────────────────────────────────

/**
 * Context passed to `getStaticParams` at build time.
 */
export interface StaticParamsContext {
  /** All route entries discovered in the app. */
  routes: Array<{ path: string; id: string }>
  /** The dynamic route currently requesting parameters. */
  route: {
    path: string
    segments: StaticParamSegment[]
  }
}

/** A dynamic segment included in the route being statically generated. */
export interface StaticParamSegment {
  name: string
  catchAll: boolean
  optional: boolean
}

/** A value captured from a Next-style dynamic route segment. */
export type RouteParamValue = string | string[] | undefined

/** Parameter object shared by pages, layouts, and route handlers. */
export type RouteParams = Record<string, RouteParamValue>

/** Duration accepted by persistent SSG parameter discovery caching. */
export type StaticParamsCacheDuration = number | `${number}${'s' | 'm' | 'h' | 'd'}`

/**
 * Static parameter values. A string shorthand is allowed for routes with one dynamic segment.
 */
export type StaticParamsValues<TParams extends RouteParams = RouteParams> = ReadonlyArray<
  TParams | string | number
>

/** Opt-in cache metadata for parameter discovery results. */
export interface CachedStaticParams<TParams extends RouteParams = RouteParams> {
  params: StaticParamsValues<TParams>
  /** Cache duration in seconds or a compact duration such as `"10m"`. */
  cache: StaticParamsCacheDuration
}

/** Value accepted from `getStaticParams` or the `staticParams` page export. */
export type StaticParamsResult<TParams extends RouteParams = RouteParams> =
  StaticParamsValues<TParams> | CachedStaticParams<TParams>

/**
 * Type for the `getStaticParams` page export used by SSG and ISR routes
 * with dynamic segments.
 *
 * @example
 * ```tsx
 * export const getStaticParams: GetStaticParams = async () => {
 *   const posts = await fetchPosts()
 *   return posts.map(post => ({ slug: post.slug }))
 * }
 * ```
 */
export type GetStaticParams<TParams extends RouteParams = RouteParams> = (
  ctx: StaticParamsContext,
) => StaticParamsResult<TParams> | Promise<StaticParamsResult<TParams>>

/**
 * Props provided to a page component during rendering.
 */
export interface PageProps<TParams extends RouteParams = RouteParams> {
  params: TParams
  requestPath: string
}

export interface MiddlewareConfig {
  builtin?: BuiltinMiddlewareConfig
  layers?: LayerConfig[]
  plugins?: MiddlewarePluginConfig[]
}

export interface BuiltinMiddlewareConfig {
  cors?: CorsConfig
  timing?: boolean
  log?: boolean
  rate?: RateLimitConfig
  headers?: Record<string, string>
}

export interface CorsConfig {
  origins?: string[]
  methods?: string[]
  headers?: string[]
  credentials?: boolean
  maxAge?: number
}

export interface RateLimitConfig {
  max: number
  window: number
  key?: string
}

export interface LayerConfig {
  kind: string
  options?: unknown
}

export interface MiddlewarePluginConfig {
  name: string
  path: string
  phase?: 'request' | 'response'
  routes?: string[]
  config?: unknown
  allow?: PluginPermissions
}

export interface PluginPermissions {
  env?: string[]
  /** Reserved: non-empty values are rejected until filesystem permissions are implemented. */
  read?: string[]
  /** Reserved: non-empty values are rejected until network permissions are implemented. */
  net?: string[]
  timeout?: number
  memory?: number
}

export interface TransformResult {
  code: string
  map?: unknown
}

export interface PluginContext {
  environment: 'client' | 'server' | 'edge' | 'worker' | 'shared'
}

export interface RuvyxaPlugin {
  name: string
  enforce?: 'pre' | 'post'
  /**
   * Allow build hooks to run in multiple isolated JavaScript workers. Enable only when every hook is
   * deterministic and does not depend on process-local mutable state. @default false
   */
  parallel?: boolean
  resolveId?(id: string): string | null | Promise<string | null>
  transform?(
    code: string,
    id: string,
    ctx: PluginContext,
  ): TransformResult | null | Promise<TransformResult | null>
}

export interface BuildContext {
  root: string
  outDir: string
  /** Override the generated chunk manifest path when an adapter relocates client output. */
  chunkManifest?: string
}

export interface AdapterOutput {
  name: string
  target: Adapter['target']
  entry: string
  assetsDir: string
  /** Directory that adapters must copy or publish with hashed client chunks. */
  clientDir?: string
  /** Chunk graph consumed by deployment tooling when `build.manifest` is enabled. */
  chunkManifest?: string
  platform?: 'node' | 'vercel' | 'cloudflare' | 'netlify' | 'bun' | 'static'
  /** Runtime expected by the deployment entrypoint. */
  runtime?: 'node' | 'bun'
  configFiles?: string[]
  functionsDir?: string
}

export interface Adapter {
  name: string
  target: 'node' | 'edge' | 'serverless' | 'static'
  build(ctx: BuildContext): AdapterOutput | Promise<AdapterOutput>
}
