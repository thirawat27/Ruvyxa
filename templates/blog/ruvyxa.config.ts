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
  // Identity for everything the build publishes about this site: `robots.txt`,
  // `sitemap.xml`, and — because `content` is enabled below — `rss.xml`,
  // `content.json`, `search-index.json`, and `llms.txt`.
  site: {
    // ⚠ Replace this before deploying. Every absolute URL in the feed, the
    // sitemap, and llms.txt is built from it, so a placeholder that ships is a
    // feed nobody can follow. The content engine below requires a value, which
    // is why this is a literal rather than left to RUVYXA_SITE_URL — an
    // unset `url` falls back to that variable, but only when nothing here
    // depends on knowing the origin at config time.
    url: 'https://example.com',
    title: 'My Ruvyxa Blog',
    description: 'Thoughts on web development, design, and building with Ruvyxa.',
    language: 'en',
  },
  // Derives the feed, the search index, the sitemap, and llms.txt from the
  // Markdown and MDX routes under `app/`. Posts are the source; nothing has to
  // be listed twice.
  content: true,
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
