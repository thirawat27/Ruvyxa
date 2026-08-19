import { config, type RuvyxaConfig } from 'ruvyxa/config'
import { realtime } from '@ruvyxa/realtime/plugin'
import { demoPlugins } from './plugins'

const settings: RuvyxaConfig = {
  appDir: 'app',
  outDir: '.ruvyxa',
  typedRoutes: true,

  server: {
    host: 'localhost',
    port: 3000,
  },

  // Set RUVYXA_SITE_URL to the real deployment origin. Without one, the build intentionally emits
  // robots.txt only instead of publishing a sitemap with fabricated URLs.
  site: {
    sitemap: {
      defaults: {
        lastModified: new Date('2026-07-29'),
        changeFrequency: 'weekly',
        priority: 0.7,
      },
      entries: [
        {
          url: '/',
          changeFrequency: 'daily',
          priority: 1,
        },
        { url: '/blog', changeFrequency: 'daily', priority: 0.9 },
        { url: '/about', changeFrequency: 'monthly', priority: 0.6 },
      ],
    },
  },

  build: {
    minify: true,
    map: false,
    treeShake: true,
    split: 'route',
    // `workers` is intentionally unset: the build sizes route bundling to the
    // machine's cores and free memory. Pinning a number here caps a 16-core
    // machine at 4 and asks a memory-limited CI container for more than it has.
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

  middleware: {
    workers: 2,
  },
  image: {
    optimize: true,
    quality: 82,
    lossless: false,
    workers: 2,
  },

  plugins: [...demoPlugins, realtime()],
}

export default config(settings)
