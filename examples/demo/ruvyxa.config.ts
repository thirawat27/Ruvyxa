import { config, type RuvyxaConfig } from 'ruvyxa/config'

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

  realtime: true,

  // Response headers by path, evaluated natively on every host. One header per
  // render strategy labels which branch of the server answered.
  headers: [
    { source: '/static-page', headers: [{ key: 'x-demo-render-mode', value: 'static' }] },
    { source: '/ssg-blog/:path*', headers: [{ key: 'x-demo-render-mode', value: 'ssg' }] },
    { source: '/isr-page', headers: [{ key: 'x-demo-render-mode', value: 'isr' }] },
    { source: '/csr-page', headers: [{ key: 'x-demo-render-mode', value: 'csr' }] },
    { source: '/ppr-page', headers: [{ key: 'x-demo-render-mode', value: 'ppr' }] },
    { source: '/proxy-lab/:path*', headers: [{ key: 'x-demo-headers-rule', value: 'active' }] },
  ],

  // Code ahead of every matching route, kept in this file. A `Response`
  // answers; a `Request` continues with new headers.
  proxy: {
    matcher: '/proxy-lab/:path*',
    handler(request) {
      const url = new URL(request.url)
      if (url.pathname === '/proxy-lab/blocked') {
        return new Response('blocked by proxy', {
          status: 403,
          headers: { 'x-demo-proxy': 'answered' },
        })
      }
      const headers = new Headers(request.headers)
      headers.set('x-demo-proxy-request', 'active')
      return new Request(request, { headers })
    },
  },
}

export default config(settings)
