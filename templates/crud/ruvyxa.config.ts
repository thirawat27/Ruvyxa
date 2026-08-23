import { config } from 'ruvyxa/config'

/**
 * Every option is optional, and every default is the one a production build
 * should already want: minified output, route-level splitting, route and CSS
 * caches on, images optimized, security headers applied. So this file holds
 * only the decisions a new project actually has to make.
 *
 * Add a key when you want something *other* than the default — restating one
 * pins it, and the pinned value is what a future release can no longer improve
 * for this project. `docs/07-configuration.md` has the full option map.
 */
export default config({
  // Generates `.ruvyxa/types/routes.d.ts`, which narrows `<Link href>`,
  // `useRouter().push`, and `useRouter().prefetch` to the routes this project
  // actually has. Off by default; the `include` in tsconfig.json is what makes
  // TypeScript read the generated file.
  typedRoutes: true,

  site: {
    // `robots.txt` and `sitemap.xml` are generated from the route manifest
    // during `ruvyxa build`. Give `url` the deployed origin — or set
    // RUVYXA_SITE_URL in the deployment environment — and the sitemap is
    // published too. Without one the build emits `robots.txt` alone rather than
    // a sitemap of invented URLs.
    // url: 'https://example.com',
  },

  // `server.host` and `server.port` are not set here on purpose. `ruvyxa dev`
  // serves localhost:3000, `ruvyxa start` binds 0.0.0.0:3000, and both read
  // HOST and PORT from the environment — which is how a container tells the
  // process which port it was given. A value written here would outrank that.
})
