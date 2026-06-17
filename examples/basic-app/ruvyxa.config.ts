import { defineConfig } from "ruvyxa/config"

export default defineConfig({
  appDir: "app",
  outDir: ".ruvyxa",
  runtime: "node",
  react: true,
  server: {
    port: 3000,
    host: "localhost",
  },
  build: {
    minify: true,
    sourcemap: true,
    splitStrategy: "route",
  },
  debug: {
    overlay: true,
    traces: true,
  },
})
