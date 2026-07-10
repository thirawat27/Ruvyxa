export interface RuvyxaConfig {
  appDir?: string
  outDir?: string
  runtime?: 'node' | 'edge' | 'static'
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
    sourcemap?: boolean
    treeShaking?: boolean
    splitStrategy?: 'single' | 'route' | 'manual'
    parallelism?: number
    jsxRuntime?: 'classic' | 'automatic'
    esTarget?: 'es2018' | 'es2019' | 'es2020' | 'es2022' | 'esnext'
    emitChunkManifest?: boolean
    /** Precompile dev route modules and load their dependencies in background workers. */
    prebundleDependencies?: boolean
  }
  rendering?: RenderingConfig
  debug?: {
    overlay?: boolean
    traces?: boolean
  }
  security?: {
    actionBodyLimitBytes?: number
    sameOriginActions?: boolean
    fetchMetadataActions?: boolean
    securityHeaders?: boolean
  }
  cache?: {
    routeManifest?: boolean
    css?: boolean
    /** Shared compile-cache directory. Relative paths are resolved from the project root. */
    buildDir?: string
  }
  middleware?: MiddlewareConfig
  adapter?: Adapter
  adapterOptions?: Record<string, unknown>
  plugins?: RuvyxaPlugin[]
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
 * Global rendering configuration in `ruvyxa.config.ts`.
 */
export interface RenderingConfig {
  /**
   * Default rendering strategy for pages that don't declare one explicitly.
   * @default "ssr"
   */
  defaultStrategy?: RenderStrategy
  /**
   * Fallback behavior for SSG/ISR pages when a path is not pre-rendered.
   * - `"blocking"` — server-render on first request, then cache (default).
   * - `"static"` — return 404 for paths not pre-rendered at build time.
   * @default "blocking"
   */
  fallback?: 'blocking' | 'static'
  /**
   * Default ISR revalidation interval in seconds (used when a page exports
   * `revalidate` without a value or inherits ISR from config).
   * @default 60
   */
  defaultRevalidate?: number
}

// ─── Per-Page Exports ─────────────────────────────────────────────────────────

/**
 * Context passed to `getStaticParams` at build time.
 */
export interface StaticParamsContext {
  /** All route entries discovered in the app. */
  routes: Array<{ path: string; id: string }>
}

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
export type GetStaticParams<TParams extends Record<string, string> = Record<string, string>> = (
  ctx: StaticParamsContext,
) => TParams[] | Promise<TParams[]>

/**
 * Props provided to a page component during rendering.
 */
export interface PageProps<TParams extends Record<string, string> = Record<string, string>> {
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
  logging?: boolean
  rateLimit?: RateLimitConfig
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
  maxRequests: number
  windowSecs: number
  keyBy?: string
}

export interface LayerConfig {
  kind: string
  options?: unknown
}

export interface MiddlewarePluginConfig {
  name: string
  path: string
  hotReload?: boolean
  phase?: 'request' | 'response'
  routes?: string[]
  config?: unknown
  permissions?: PluginPermissions
}

export interface PluginPermissions {
  env?: string[]
  fsRead?: string[]
  net?: string[]
  timeoutMs?: number
  maxMemoryBytes?: number
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
  /** Chunk graph consumed by deployment tooling when `emitChunkManifest` is enabled. */
  chunkManifest?: string
  platform?: 'node' | 'vercel' | 'cloudflare' | 'netlify' | 'bun' | 'static'
  configFiles?: string[]
  functionsDir?: string
}

export interface Adapter {
  name: string
  target: 'node' | 'edge' | 'serverless' | 'static'
  build(ctx: BuildContext): AdapterOutput | Promise<AdapterOutput>
}
