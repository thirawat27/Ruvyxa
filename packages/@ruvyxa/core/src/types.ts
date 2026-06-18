export interface RuvyxaConfig {
  appDir?: string
  outDir?: string
  runtime?: "node" | "edge" | "static"
  react?: boolean
  typescript?: {
    strict?: boolean
  }
  css?: {
    modules?: boolean
    nesting?: boolean
  }
  server?: {
    port?: number
    host?: string
  }
  build?: {
    minify?: boolean
    sourcemap?: boolean
    splitStrategy?: "route" | "manual"
    parallelism?: number
  }
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
  }
  middleware?: MiddlewareConfig
  adapter?: Adapter
  adapterOptions?: Record<string, unknown>
  plugins?: RuvyxaPlugin[]
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
  phase?: "request" | "response"
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
  environment: "client" | "server" | "edge" | "worker" | "shared"
}

export interface RuvyxaPlugin {
  name: string
  enforce?: "pre" | "post"
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
}

export interface AdapterOutput {
  name: string
  target: Adapter["target"]
  entry: string
  assetsDir: string
  platform?: "node" | "vercel" | "cloudflare" | "netlify" | "bun" | "static"
  configFiles?: string[]
  functionsDir?: string
}

export interface Adapter {
  name: string
  target: "node" | "edge" | "serverless" | "static"
  build(ctx: BuildContext): AdapterOutput | Promise<AdapterOutput>
}
