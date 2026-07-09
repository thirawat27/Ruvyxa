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
    treeShaking: true,
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
}

export default defineConfig(config)
