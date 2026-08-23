import { config, type RuvyxaConfig } from 'ruvyxa/config'

const settings: RuvyxaConfig = {
  appDir: 'app',
  outDir: '.ruvyxa',
  // Generates .ruvyxa/types/routes.d.ts, which narrows `<Link href>` and
  // `useRouter().push` to the routes this project actually has. The tsconfig
  // `include` is what makes TypeScript read it.
  typedRoutes: true,
  server: {
    host: 'localhost',
    port: 3000,
  },
  // `robots.txt` and `sitemap.xml` are generated from the route manifest during
  // `ruvyxa build`. Give `url` the deployed origin — or set RUVYXA_SITE_URL in
  // the deployment environment — and the sitemap is published too.
  site: {
    // url: 'https://example.com',
  },
  security: {
    // Largest JSON body an API route will accept, in bytes. The default is
    // 10 MB; this API only ever reads a name and a description, and a limit
    // that matches what the endpoints use is a limit an attacker cannot spend.
    apiLimit: 64 * 1024,
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
  cache: {
    routes: true,
    css: true,
  },
  debug: {
    overlay: true,
  },
  image: {
    optimize: true,
    quality: 82,
    lossless: false,
  },
}

export default config(settings)
