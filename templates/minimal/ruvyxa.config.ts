import { defineConfig, type RuvyxaConfig } from "ruvyxa/config"

const config: RuvyxaConfig = {
  appDir: "app",
  outDir: ".ruvyxa",

  server: {
    host: "localhost",
    port: 3000,
  },

  build: {
    minify: true,
    sourcemap: false,
    splitStrategy: "route",
    parallelism: 4,
  },

  cache: {
    routeManifest: true,
    css: true,
  },

  debug: {
    overlay: true,
    traces: true,
  },

  // Middleware configuration — tower-based layers applied to every request.
  middleware: {
    builtin: {
      // Response timing header (X-Response-Time)
      timing: true,
      // Request logging (method, path, status, duration)
      logging: true,
      // CORS — uncomment to enable:
      // cors: {
      //   origins: ["http://localhost:5173"],
      //   methods: ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
      //   headers: ["Content-Type", "Authorization"],
      //   credentials: true,
      //   maxAge: 86400,
      // },
      // Rate limiting — uncomment to enable:
      // rateLimit: {
      //   maxRequests: 100,
      //   windowSecs: 60,
      //   keyBy: "ip",
      // },
      // Custom response headers:
      // headers: {
      //   "X-Powered-By": "Ruvyxa",
      // },
    },
    // Wasm plugins — sandboxed WebAssembly modules for request/response interception.
    // Each plugin runs in an isolated Wasmtime sandbox with configurable permissions.
    // plugins: [
    //   {
    //     name: "auth-guard",
    //     path: "plugins/auth-guard.wasm",
    //     phase: "request",
    //     hotReload: true,
    //     routes: ["/api/*", "/dashboard/*"],
    //     config: { apiKeyHeader: "X-Api-Key" },
    //     permissions: {
    //       env: ["AUTH_SECRET"],
    //       timeoutMs: 5000,
    //       maxMemoryBytes: 67108864,
    //     },
    //   },
    // ],
  },

  // Optional knobs:
  // runtime: "node",
  // react: true,
  // typescript: { strict: true },
  // css: { modules: false, nesting: true },
  // security: {
  //   actionBodyLimitBytes: 64 * 1024,
  //   sameOriginActions: true,
  //   fetchMetadataActions: true,
  //   securityHeaders: true,
  // },
  // adapter: nodeAdapter(),
}

export default defineConfig(config)
