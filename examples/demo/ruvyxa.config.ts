import { config, type RuvyxaConfig } from 'ruvyxa/config'

const settings: RuvyxaConfig = {
  appDir: 'app',
  outDir: '.ruvyxa',

  server: {
    host: 'localhost',
    port: 3000,
  },

  build: {
    minify: true,
    map: false,
    treeShake: true,
    split: 'route',
    workers: 4,
  },

  render: {
    strategy: 'ssr',
    revalidate: 60,
  },

  cache: {
    routes: true,
    css: true,
  },

  debug: {
    overlay: true,
    traces: true,
  },
  image: {
    optimize: true,
    quality: 82,
    lossless: false,
    workers: 0,
  },
}

export default config(settings)
