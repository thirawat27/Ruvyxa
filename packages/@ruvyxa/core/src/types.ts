import type { DeployManifest } from './deploy-manifest.js'

/**
 * Accepted values for `build.target`.
 *
 * The canonical spellings only. `es6` and mixed case are tolerated at runtime,
 * and `es5` is absent because oxc does not implement it. The authority is
 * `tests/fixtures/es-target-conformance.json`, which both compilers replay.
 */
export type EsTarget =
  | 'es2015'
  | 'es2016'
  | 'es2017'
  | 'es2018'
  | 'es2019'
  | 'es2020'
  | 'es2021'
  | 'es2022'
  | 'es2023'
  | 'es2024'
  | 'es2025'
  | 'es2026'
  | 'esnext'

export interface RuvyxaConfig {
  appDir?: string
  outDir?: string
  /** Runtime used for config, rendering, and plugins. @default 'node' */
  runtime?: 'node' | 'bun' | 'deno' | 'edge' | 'static'
  /**
   * Run the stable React Compiler in inference mode before Ruvyxa's Oxc
   * transform for production builds. Disabled unless explicitly enabled.
   * @default false
   */
  reactCompiler?: boolean
  /**
   * Generate `.ruvyxa/types/routes.d.ts` so `<Link href>`, `useRouter().push`,
   * and `useRouter().prefetch` are checked against the routes that exist.
   *
   * `ruvyxa dev` rewrites the file whenever route discovery re-runs;
   * `ruvyxa build` and `ruvyxa check` write it once. The file only takes effect
   * if `tsconfig.json` includes it, so a project enabling this must also add
   * `".ruvyxa/types/**\/*.d.ts"` to `include` — `ruvyxa check` says so if it
   * is missing.
   *
   * @default false
   */
  typedRoutes?: boolean
  css?: {
    /** Additional project-relative global stylesheet files or directories. */
    entries?: string[]
  }
  /**
   * Markdown and MDX compilation powered by `@mdx-js/mdx`.
   *
   * Plugins run for both `.md` and `.mdx` routes in the configured order.
   * Ruvyxa's metadata and safety transforms run after application plugins so
   * exported headings match rendered IDs and raw HTML in `.md` stays inert.
   */
  markdown?: MarkdownConfig
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
    /**
     * JavaScript language level the emitted modules are written down to.
     *
     * Applied by both compilers — the Rust client graph and
     * `runtime/compiler.mjs` for the server and prerender graph — and held to
     * one accepted list by `tests/fixtures/es-target-conformance.json`.
     *
     * A target below the syntax the source uses can require
     * `@oxc-project/runtime` helpers, and Ruvyxa ships no helper runtime, so a
     * module that would need one fails the build by name rather than emitting
     * an import nothing can resolve. Ordinary application code is helper-free
     * at `es2022` and above.
     *
     * `es6` is accepted as an alias for `es2015`, and values are matched
     * case-insensitively after trimming.
     *
     * @default 'esnext'
     */
    target?: EsTarget
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
  i18n?: I18nConfig
  security?: {
    /** Maximum server-action payload size in bytes. @default 1048576 */
    actionLimit?: number
    /** Maximum API route request payload size in bytes. @default 10485760 */
    apiLimit?: number
    /**
     * Maximum response size buffered by TypeScript response middleware in bytes.
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
     * Non-loopback reverse proxies allowed to supply X-Forwarded-For,
     * X-Real-IP, and X-Forwarded-Proto. Loopback proxies are trusted by
     * default.
     *
     * Each entry is either an exact address (`10.0.0.9`, `2001:db8::2`) or a
     * CIDR range (`10.0.0.0/8`, `2001:db8::/32`). Ranges are what container
     * networks and managed platform edges need, because their proxy address is
     * not stable enough to enumerate.
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
  site?: SiteConfig
  /**
   * Content-derived application artifacts. Markdown and MDX page routes do not
   * require this setting; enable it only when the application also needs a
   * content manifest, search index, RSS feed, sitemap, or llms.txt.
   */
  content?: boolean | ContentConfig
  middleware?: MiddlewareConfig
  adapter?: Adapter
  adapterOptions?: Record<string, unknown>
  plugins?: RuvyxaPlugin[]
}

/** A unified-compatible plugin or preset accepted by the MDX compiler. */
export type MarkdownPlugin = ((...parameters: never[]) => unknown) | MarkdownPluginPreset

/** A unified preset groups plugins and optional shared settings. */
export interface MarkdownPluginPreset {
  plugins: MarkdownPluginList
  settings?: Record<string, unknown>
}

/** A plugin alone or a plugin followed by its configuration arguments. */
export type MarkdownPluginEntry =
  MarkdownPlugin | readonly [MarkdownPlugin, ...configuration: unknown[]]

/** Ordered unified plugins for one compiler stage. */
export type MarkdownPluginList = readonly MarkdownPluginEntry[]

/** Extensible Markdown/MDX compiler options shared by dev and production. */
export interface MarkdownConfig {
  /** Enable GitHub Flavored Markdown tables, task lists, autolinks, and footnotes. @default true */
  gfm?: boolean
  /** Transform the Markdown AST before it becomes HTML. */
  remarkPlugins?: MarkdownPluginList
  /** Transform the HTML AST before React code is emitted. */
  rehypePlugins?: MarkdownPluginList
  /** Transform the generated JavaScript ESTree. */
  recmaPlugins?: MarkdownPluginList
  /** Options forwarded from remark to rehype, such as localized footnote labels. */
  remarkRehypeOptions?: Record<string, unknown>
}

/**
 * Site identity used by the crawler discovery files the build emits.
 *
 * `robots.txt` and `sitemap.xml` are generated from the route manifest during
 * `ruvyxa build`. A file of the same name in `public/`, or an exact application
 * route, suppresses the core generator for that path.
 */
export interface SiteConfig {
  /**
   * Actual absolute origin of the deployed site.
   *
   * Falls back to `RUVYXA_SITE_URL`, then to the production URL exported by
   * Vercel or Netlify. Preview-only deployment URLs are never selected as a
   * canonical origin. A bare hostname is normalized to `https`.
   */
  url?: string
  /** Shared site title used by content-derived artifacts. */
  title?: string
  /** Shared site description used by content-derived artifacts. */
  description?: string
  /** BCP 47 language used by feeds and content tokenization. */
  language?: string
  /** Emit the route-derived sitemap or customize its path set. @default true */
  sitemap?: boolean | SiteSitemapConfig
  /** Emit `robots.txt` or provide a crawler policy. @default true */
  robots?: boolean | SiteRobotsConfig
}

/** Options for the content artifacts generated from native Markdown/MDX routes. */
export interface ContentConfig {
  /** Enable the standard content engine or customize its generated artifacts. */
  engine?: boolean | ContentEngineConfig
}

/** Content-engine options whose site identity comes from the shared `site` block. */
export interface ContentEngineConfig {
  /** Exact route paths or trailing-`*` patterns omitted from every artifact. */
  exclude?: string[]
  /** BCP 47 locale used for search tokenization. Defaults to `site.language`. */
  locale?: string
  stopWords?: string[]
  /** Ignore shorter search terms. @default 2 */
  minTermLength?: number
  /** @default "/content.json" */
  manifestPath?: string
  /** @default "/search-index.json" */
  searchPath?: string
  /** @default "/rss.xml" */
  feedPath?: string
  /** @default "/sitemap.xml" */
  sitemapPath?: string
  /** Agent discovery index in llms.txt format. Set false to disable. @default "/llms.txt" */
  llmsPath?: string | false
  /** Feed language. Defaults to `site.language`. */
  language?: string
}

/** Production controls for Ruvyxa's route-derived sitemap. */
export interface SiteSitemapConfig {
  /** Exact paths or trailing-`*` prefixes omitted from the sitemap. */
  exclude?: string[]
  /** Concrete dynamic URLs that route discovery or prerendering cannot infer. */
  additionalPaths?: string[]
  /** Metadata inherited by every automatically discovered and explicit entry. */
  defaults?: SiteSitemapEntryDefaults
  /** Next-style entries that enrich discovered URLs or add new URLs. */
  entries?: SiteSitemapEntry[]
}

export type SiteSitemapChangeFrequency =
  'always' | 'hourly' | 'daily' | 'weekly' | 'monthly' | 'yearly' | 'never'

/** Metadata hints that may be shared across sitemap entries. */
export interface SiteSitemapEntryDefaults {
  lastModified?: string | Date
  changeFrequency?: SiteSitemapChangeFrequency
  /** Search priority from 0 to 1. */
  priority?: number
}

/** One Next-style sitemap URL entry. Root-relative URLs resolve against `site.url`. */
export interface SiteSitemapEntry extends SiteSitemapEntryDefaults {
  url: string
  alternates?: {
    languages?: Record<string, string>
  }
  images?: string[]
  videos?: SiteSitemapVideo[]
}

export interface SiteSitemapVideoRelationship {
  relationship: 'allow' | 'deny'
  content: string
}

export interface SiteSitemapVideoUploader {
  content: string
  info?: string
}

/** Video sitemap fields supported by Next.js metadata routes. */
export interface SiteSitemapVideo {
  title: string
  thumbnail_loc: string
  description: string
  content_loc?: string
  player_loc?: string
  duration?: number
  view_count?: number
  rating?: number
  expiration_date?: string | Date
  publication_date?: string | Date
  family_friendly?: 'yes' | 'no'
  requires_subscription?: 'yes' | 'no'
  live?: 'yes' | 'no'
  restriction?: SiteSitemapVideoRelationship
  platform?: SiteSitemapVideoRelationship
  uploader?: SiteSitemapVideoUploader
  tag?: string | string[]
}

/** One robots.txt rule group. String arrays follow Next.js metadata semantics. */
export interface SiteRobotsRule {
  /** Crawler product token or tokens. @default "*" */
  userAgent?: string | string[]
  allow?: string | string[]
  disallow?: string | string[]
  crawlDelay?: number
}

/** RFC 9309 crawler rules plus widely supported sitemap and host records. */
export interface SiteRobotsConfig {
  /** One rule or multiple rule groups. Defaults to allowing all crawlers. */
  rules?: SiteRobotsRule | SiteRobotsRule[]
  /** Absolute sitemap URL or URLs. Defaults to the generated root sitemap. */
  sitemap?: string | string[]
  /** Preferred absolute site origin written as a `Host:` record. */
  host?: string
}

export interface ImageConfig {
  /** Convert local PNG/JPEG public assets to WebP during production builds. @default true */
  optimize?: boolean
  /** Lossy WebP quality from 1 to 100. @default 82 */
  quality?: number
  /** Use lossless WebP encoding; `quality` then controls encoder effort. @default false */
  lossless?: boolean
  /**
   * Publish the original PNG/JPEG next to its WebP output so a plain
   * `<img src="/logo.png">` keeps working on static hosts. By default only the
   * converted WebP is published; use `<Image>` or a `.webp` URL. @default false
   */
  keepOriginal?: boolean
  /**
   * Opt-in responsive breakpoint widths, in pixels. For each public PNG/JPEG
   * the build emits a downscaled `name-<w>w.webp` at every width narrower than
   * the source. Reference opt-in variants with an explicit `srcSet`; static
   * `<Image>` otherwise uses the single full-size WebP. @default []
   */
  variantWidths?: number[]
  /**
   * Largest width the primary WebP is encoded at, in pixels. `0` publishes the
   * source's own resolution.
   *
   * Encoding cannot be split across threads, so the primary output alone sets
   * the wall time of a large-image build: a 6000x4000 camera original takes
   * ~745ms uncapped and ~296ms at this default. 3840 is the top of the standard
   * responsive ladder and the width of a 4K display, so a wider file is bytes
   * no viewport can use. @default 3840
   */
  maxWidth?: number
  /** Image conversion workers. Zero selects the available CPU count. @default 0 */
  workers?: number
  /**
   * WebP encoder effort, 0 (fastest, largest files) to 6 (slowest, smallest).
   *
   * Encoding is the floor on build time for large images: it cannot be split
   * across threads, so once `maxWidth` has bounded the work this is the only
   * lever left. It costs bytes — measured on a 24 MP source, effort 2 is ~2.4x
   * faster for ~4% more output, effort 0 is ~3.5x faster for ~14% more. Raising
   * it to 6 costs 24% more time for no smaller output at all.
   *
   * Results are cached by source and settings, so this mostly affects cold
   * builds and CI. @default 4
   */
  effort?: number
  /** Resize same-origin files from `public/` through the runtime image endpoint. */
  onDemand?: boolean | OnDemandImageConfig
}

export interface OnDemandImageConfig {
  /** Enable runtime image transforms. @default true when this object is present */
  enabled?: boolean
  /** Largest accepted output width. @default 3840 */
  maxWidth?: number
}

/** File-system locale routing for routes such as `app/[lang]/about/page.tsx`. */
export interface I18nConfig {
  /** Supported BCP-47-style locale identifiers. */
  locales: string[]
  /** Fallback locale and the target used by the `x-default` alternate link. */
  defaultLocale: string
  /** Dynamic route parameter that carries the locale. @default "lang" */
  localeParam?: string
  /** Detect a preferred locale from cookie and Accept-Language. @default true */
  detectLocale?: boolean
  /** Locale preference cookie name. @default "RUVYXA_LOCALE" */
  cookie?: string
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
  /**
   * TypeScript plugin middleware worker processes (1-8). Workers do not share
   * module-level plugin state, so keep the default unless plugin middleware is
   * stateless and a proven throughput bottleneck.
   * @default 1
   */
  workers?: number
  /**
   * Maximum duration of one TypeScript middleware hook before its worker is
   * replaced. Timed-out hooks are not retried because they may have produced
   * side effects before stalling.
   * @default 30000
   * @maximum 300000
   */
  timeoutMs?: number
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

export interface TransformResult {
  code: string
  map?: unknown
}

/** Execution target exposed to build plugin hooks. */
export type PluginEnvironment = 'client' | 'server' | 'edge' | 'worker' | 'shared'

export interface PluginTransformContext {
  /** Absolute application root. */
  root: string
  environment: 'client' | 'server' | 'edge' | 'worker' | 'shared'
}

/** Exact pathname or a prefix pattern ending in `*`. */
export type PluginRoutePattern = string

export interface PluginHttpContext {
  /** Name of the plugin that registered this hook. */
  readonly plugin: string
  /** Absolute application root. */
  readonly root: string
}

export interface PluginHttpRequestContext extends PluginHttpContext {
  readonly request: Request
  /** Continue to the next hook, optionally with a replacement request. */
  next(request?: Request): void
}

export interface PluginHttpResponseContext extends PluginHttpContext {
  readonly request: Request
  readonly response: Response
  /** Continue to the next hook, optionally with a replacement response. */
  next(response?: Response): void
}

export type PluginHttpRequestHandler = (
  context: PluginHttpRequestContext,
) => Request | Response | void | Promise<Request | Response | void>

export type PluginHttpResponseHandler = (
  context: PluginHttpResponseContext,
) => Response | void | Promise<Response | void>

export interface PluginHttpRequestRegistration {
  /** Omit to match every application path. */
  match?: readonly PluginRoutePattern[]
  handler: PluginHttpRequestHandler
}

export interface PluginHttpResponseRegistration {
  /** Omit to match every application path. */
  match?: readonly PluginRoutePattern[]
  handler: PluginHttpResponseHandler
}

export interface PluginHttpRouteContext extends PluginHttpContext {
  readonly request: Request
}

export interface PluginHttpRouteRegistration {
  /** Exact application path. */
  path: string
  /** One method, several methods, or every method when omitted. */
  method?: string | readonly string[]
  handler(context: PluginHttpRouteContext): Response | Promise<Response>
}

export interface PluginHttpSocket {
  onRequest(registration: PluginHttpRequestRegistration | PluginHttpRequestHandler): void
  onResponse(registration: PluginHttpResponseRegistration | PluginHttpResponseHandler): void
  route(registration: PluginHttpRouteRegistration): void
}

/** Concise HTTP declarations accepted by `definePlugin`. */
export interface PluginHttpDefinition {
  /** Optional scope shared by `onRequest`, `onResponse`, and generated response headers. */
  match?: readonly PluginRoutePattern[]
  onRequest?: PluginHttpRequestHandler
  onResponse?: PluginHttpResponseHandler
  routes?: readonly PluginHttpRouteRegistration[]
}

export interface PluginBuildResolveContext extends PluginTransformContext {
  readonly id: string
  readonly importer?: string
}

export type PluginBuildResolveHandler = (
  context: PluginBuildResolveContext,
) => string | null | void | Promise<string | null | void>

export interface PluginBuildLoadContext extends PluginTransformContext {
  readonly id: string
}

export type PluginBuildLoadHandler = (
  context: PluginBuildLoadContext,
) => string | TransformResult | null | void | Promise<string | TransformResult | null | void>

export interface PluginBuildTransformContext extends PluginTransformContext {
  readonly code: string
  readonly id: string
}

export type PluginBuildTransformHandler = (
  context: PluginBuildTransformContext,
) => string | TransformResult | null | void | Promise<string | TransformResult | null | void>

export interface PluginBuildContext {
  /** Absolute application root. */
  root: string
  /** Absolute build output directory. */
  outDir: string
  /** Parsed application build manifest. */
  manifest: Readonly<Record<string, unknown>>
}

export type PluginBuildCompleteHook = (context: PluginBuildContext) => void | Promise<void>

export interface PluginBuildStartContext {
  readonly root: string
  readonly outDir: string
}

export type PluginBuildStartHook = (context: PluginBuildStartContext) => void | Promise<void>

export interface PluginBuildSocket {
  onStart(hook: PluginBuildStartHook): void
  onResolve(hook: PluginBuildResolveHandler): void
  onLoad(hook: PluginBuildLoadHandler): void
  onTransform(hook: PluginBuildTransformHandler): void
  onComplete(hook: PluginBuildCompleteHook): void
}

/** Concise build declarations accepted by `definePlugin`. Use `register` for repeated hooks. */
export interface PluginBuildDefinition {
  onStart?: PluginBuildStartHook
  onResolve?: PluginBuildResolveHandler
  onLoad?: PluginBuildLoadHandler
  onTransform?: PluginBuildTransformHandler
  onComplete?: PluginBuildCompleteHook
}

/** Native self-hosted realtime transport requested by a first-party plugin. */
export interface RealtimePluginOptions {
  /** WebSocket endpoint. Must be an absolute application path. @default "/__ruvyxa/realtime" */
  path?: string
  /** WebSocket heartbeat interval in milliseconds. @default 25000 */
  heartbeatMs?: number
  /** Per-process broadcast queue capacity. @default 256 */
  capacity?: number
}

/**
 * Native self-hosted collaboration transport requested by a first-party plugin.
 *
 * Unlike {@link RealtimePluginOptions}, this transport is bidirectional: peers
 * publish presence and write shared state, and the server sequences every
 * write. Rooms are process-local and ephemeral, so a multi-process deployment
 * gives each process its own rooms.
 */
export interface PresencePluginOptions {
  /** WebSocket endpoint. Must be an absolute application path. @default "/__ruvyxa/collab" */
  path?: string
  /** WebSocket heartbeat interval in milliseconds. @default 25000 */
  heartbeatMs?: number
}

export interface PluginDevFileChangeContext {
  readonly root: string
  readonly paths: readonly string[]
}

export type PluginDevFileChangeHandler = (
  context: PluginDevFileChangeContext,
) => void | Promise<void>

export interface PluginDevFileChangeRegistration {
  /** Optional path patterns, relative to the application root. */
  match?: readonly string[]
  handler: PluginDevFileChangeHandler
}

export interface PluginDevSocket {
  onFileChange(registration: PluginDevFileChangeRegistration | PluginDevFileChangeHandler): void
}

/** Concise development declarations accepted by `definePlugin`. */
export interface PluginDevDefinition {
  onFileChange?: PluginDevFileChangeRegistration | PluginDevFileChangeHandler
}

export type PluginDiagnosticLevel = 'info' | 'warning' | 'error'

export interface PluginDiagnostic {
  level: PluginDiagnosticLevel
  code: string
  message: string
}

export interface PluginDiagnosticsSocket {
  report(diagnostic: PluginDiagnostic): void
}

export type PluginNativeCapability = 'realtime@1' | 'presence@1'

export interface PluginNativeSocket {
  claim(capability: 'realtime@1', options?: RealtimePluginOptions): void
  claim(capability: 'presence@1', options?: PresencePluginOptions): void
}

/** Framework-owned native capabilities requested declaratively by a plugin. */
export interface PluginNativeDefinition {
  /** Enable the self-hosted realtime capability; `true` uses its defaults. */
  realtime?: RealtimePluginOptions | true
  /** Enable the self-hosted collaboration capability; `true` uses its defaults. */
  presence?: PresencePluginOptions | true
}

/** Grouped extension sockets available while a plugin registers itself. */
/**
 * Which environment the plugin host is serving.
 *
 * Unrelated to `PluginEnvironment`, which names the bundle a build hook is
 * transforming for (`client`, `server`, `edge`, ...). This is `ruvyxa dev`
 * versus a server or function answering real traffic.
 */
export type PluginHostEnvironment = 'development' | 'production'

export interface PluginRegistrationApi {
  /**
   * The environment this host serves.
   *
   * Available at registration rather than per request on purpose: a plugin
   * that only makes sense in one environment declines to register its hooks at
   * all, so the other environment pays nothing. A host that does not state an
   * environment reports `production`, so development-only behaviour is never
   * enabled by omission.
   */
  readonly environment: PluginHostEnvironment
  readonly http: PluginHttpSocket
  readonly build: PluginBuildSocket
  readonly dev: PluginDevSocket
  readonly diagnostics: PluginDiagnosticsSocket
  readonly native: PluginNativeSocket
}

/**
 * One element a plugin contributes to every rendered document's `<head>`.
 *
 * Declared once at config load and injected by the server, so a plugin adds an
 * analytics snippet, a preconnect, or a verification tag without paying for a
 * per-request round trip into the plugin host.
 *
 * Only elements that are legal in `<head>` are accepted, and attribute values
 * are HTML-escaped. To contribute per-route metadata instead, export `meta`
 * from the route — a plugin cannot know which route is rendering.
 */
export interface PluginHeadEntry {
  tag: 'link' | 'meta' | 'noscript' | 'script' | 'style'
  /** Attribute names and values. Values are escaped before they are written. */
  attrs?: Record<string, string | number | boolean>
  /**
   * Text content for `script`, `style`, and `noscript`.
   *
   * Written verbatim: these elements have raw-text content models, so escaping
   * would corrupt them. A plugin is trusted project code — do not build this
   * string from untrusted input.
   */
  children?: string
}

/**
 * Input accepted by `definePlugin`.
 *
 * Prefer concise declarations for common behavior. `register(api)` remains the escape hatch for
 * multiple hooks of the same kind or advanced composition.
 */
export interface RuvyxaPluginDefinition {
  name: string
  headers?: HeadersInit
  head?: PluginHeadEntry | readonly PluginHeadEntry[]
  http?: PluginHttpDefinition
  build?: PluginBuildDefinition
  dev?: PluginDevDefinition
  diagnostics?: PluginDiagnostic | readonly PluginDiagnostic[]
  native?: PluginNativeDefinition
  register?(api: PluginRegistrationApi): void | Promise<void>
}

/** The sole plugin object accepted by `config({ plugins })`. */
export interface RuvyxaPlugin {
  readonly name: string
  /** Head elements this plugin contributes to every rendered document. */
  readonly head?: readonly PluginHeadEntry[]
  register(api: PluginRegistrationApi): void | Promise<void>
}

export interface BuildContext {
  root: string
  outDir: string
  /** Override the generated chunk manifest path when an adapter relocates client output. */
  chunkManifest?: string
  /**
   * Read-only metadata emitted by the Ruvyxa build. Adapters use this to carry
   * validated runtime policy (for example middleware and i18n) into generated
   * serverless/edge handlers without evaluating project config a second time.
   */
  buildInfo?: Readonly<Record<string, unknown>>
  /**
   * The `deploy` section of the build's `manifest.json`, read once by the
   * adapter runner.
   *
   * Says which routes may be answered from a file, which must reach the
   * function, and what cache-control each class of emitted file carries.
   * Adapters used to re-derive all three from route metadata, one copy each,
   * and the copies disagreed. `undefined` when the output came from a Ruvyxa
   * older than the manifest, or newer than this package understands; the
   * helpers in `deploy-manifest.ts` all fall back to deriving it, so an adapter
   * keeps working either way.
   */
  deployManifest?: DeployManifest | null
}

/** The platforms the adapters in this repository target. */
export type AdapterPlatform =
  | 'node'
  | 'vercel'
  | 'cloudflare'
  | 'netlify'
  | 'bun'
  | 'deno'
  | 'static'
  | 'railway'
  | 'render'
  | 'firebase'
  | 'aws'

export interface AdapterOutput {
  name: string
  target: Adapter['target']
  entry: string
  assetsDir: string
  /** Directory that adapters must copy or publish with hashed client chunks. */
  clientDir?: string
  /** Chunk graph consumed by deployment tooling when `build.manifest` is enabled. */
  chunkManifest?: string
  /**
   * The hosting platform this output targets.
   *
   * The official names autocomplete; any other string is accepted, because a
   * third-party adapter targets a platform this package has never heard of and
   * a closed union left it with nothing honest to write here. `ruvyxa build
   * --adapter <package>` has always resolved an arbitrary adapter package, so
   * the type is what was closed, not the mechanism.
   */
  platform?: AdapterPlatform | (string & {})
  /** Runtime expected by the deployment entrypoint. */
  runtime?: 'node' | 'bun' | 'deno'
  configFiles?: string[]
  functionsDir?: string
  /**
   * Declarative post-build artifacts materialized by the Ruvyxa CLI inside the
   * atomic build staging directory. Paths must be relative to the build output.
   */
  artifacts?: AdapterArtifact[]
}

/** Read-only capability report returned by the adapter inspection protocol. */
export interface AdapterInspection {
  name: string
  target: Adapter['target'] | 'unknown'
  runtime: 'node' | 'bun' | 'deno' | 'edge' | 'static' | 'unknown'
  platform?: AdapterOutput['platform'] | null
  supports: Array<RenderStrategy | 'api'>
}

/** A post-build file or static deployment bundle requested by an adapter. */
export interface AdapterArtifact {
  /**
   * - `'file'` — write a UTF-8 file.
   * - `'static-site'` — assemble a static-only publish directory from
   *   pre-rendered pages and client assets.
   * - `'function'` — bundle a serverless/edge function from a handler entry
   *   point and a compiled static route registry.
   */
  kind: 'file' | 'static-site' | 'function'
  /**
   * Destination relative to the Ruvyxa output directory (`scope: 'build'`,
   * the default) or the project root (`scope: 'project'`).
   */
  path: string
  /** Required UTF-8 contents for `file` artifacts. */
  contents?: string
  /**
   * Handler entry source code for `function` artifacts. This is the
   * platform-specific wrapper that imports `serverless-handler.mjs` plus the
   * generated `route-modules.mjs` registry and adapts them to the platform's
   * function signature.
   */
  handlerSource?: string
  /**
   * Where the artifact is materialized. Project-scope paths are restricted to
   * an allowlist of hosting-platform locations (for example `.vercel/output`
   * or `netlify.toml`) so adapters cannot write arbitrary project files.
   * @default 'build'
   */
  scope?: 'build' | 'project'
  /**
   * Skip writing a project-scope `file` artifact when the destination already
   * exists, so user-authored platform config always wins.
   * @default false
   */
  skipIfExists?: boolean
  /**
   * For `static-site` artifacts: tolerate a build without pre-rendered pages
   * (for example an API-only app) instead of failing with RUV2202. Assets and
   * client bundles are still assembled.
   * @default false
   */
  optional?: boolean
  /**
   * For `static-site` artifacts: rendering strategies whose pre-rendered
   * pages must stay out of the publish directory.
   *
   * A host that serves a matching static file before invoking the function
   * (Vercel's `handle: filesystem`, Netlify's `preferStatic`) would otherwise
   * pin an ISR page to its build-time snapshot forever — the function that
   * owns revalidation is never reached, silently turning ISR and PPR into SSG.
   * The deploy-time HTML still ships inside the function bundle, which reads
   * it as the pre-revalidation cache entry.
   */
  excludeStrategies?: string[]
}

export interface Adapter {
  name: string
  target: 'node' | 'edge' | 'serverless' | 'static'
  /**
   * Rendering strategies and route kinds this adapter supports. When set, the
   * adapter runner validates each route against this list and rejects
   * unsupported strategies with a per-route error instead of the
   * all-or-nothing RUV2202.
   *
   * Omit to support all strategies (full-featured adapters) or set to
   * `['ssg', 'csr']` for static-only adapters.
   */
  supports?: Array<RenderStrategy | 'api'>
  build(ctx: BuildContext): AdapterOutput | Promise<AdapterOutput>
}
