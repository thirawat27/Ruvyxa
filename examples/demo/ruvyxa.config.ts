import { defineConfig, type RuvyxaConfig } from 'ruvyxa/config'

const config: RuvyxaConfig = {
  appDir: 'app',
  outDir: '.ruvyxa',

  server: {
    host: 'localhost',
    port: 3000,
  },

  build: {
    minify: true,
    sourcemap: false,
    treeShaking: true,
    splitStrategy: 'route',
    parallelism: 4,
  },

  rendering: {
    defaultStrategy: 'ssr',
    fallback: 'blocking',
    defaultRevalidate: 60,
  },

  cache: {
    routeManifest: true,
    css: true,
  },

  debug: {
    overlay: true,
    traces: true,
  },
  images: {
    optimize: true,
    quality: 82,
    lossless: false,
    parallelism: 0,
  },
}

export default defineConfig(config)
