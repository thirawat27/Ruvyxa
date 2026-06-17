import { defineConfig } from "ruvyxa/config"

export default defineConfig({
  appDir: "app",
  outDir: ".ruvyxa",
  server: {
    port: 3000,
    host: "localhost",
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
})
