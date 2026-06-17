import { defineConfig } from "ruvyxa/config"

export default defineConfig({
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
  // plugins: [],
})
