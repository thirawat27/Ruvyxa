/**
 * First-party Ruvyxa plugins, ready to drop into `ruvyxa.config.ts`:
 *
 * ```ts
 * import { redirects, headers, sitemap, robots, alias } from 'ruvyxa/plugins'
 *
 * export default config({
 *   plugins: [
 *     redirects([{ source: '/old-blog/*', destination: '/blog/*', permanent: true }]),
 *     headers([{ source: '/api/*', headers: { 'cache-control': 'no-store' } }]),
 *     sitemap({ siteUrl: 'https://example.com', robots: true }),
 *   ],
 * })
 * ```
 *
 * Every plugin uses only the public plugin API, so custom plugins can compose
 * with them freely. Route patterns follow middleware semantics: `*` matches
 * everything, a trailing `*` matches by prefix, anything else matches exactly.
 */

export { cacheRules, headers, observability, redirects, securityHeaders } from './plugins/http.js'
export type {
  CacheRule,
  ContentSecurityPolicy,
  HeaderRule,
  ObservabilityEntry,
  ObservabilityOptions,
  RedirectRule,
  SecurityHeadersOptions,
} from './plugins/http.js'
export { pwa } from './plugins/pwa.js'
export type { PwaIcon, PwaOptions } from './plugins/pwa.js'
export { feed, robots, sitemap } from './plugins/seo.js'
export type {
  FeedItem,
  FeedOptions,
  RobotsOptions,
  RobotsRule,
  SitemapOptions,
} from './plugins/seo.js'
export { searchIndex } from './plugins/search.js'
export type { SearchDocument, SearchIndexOptions } from './plugins/search.js'
export { contentEngine, contentEngineFromConfig } from './plugins/content-engine.js'
export type {
  ContentEngineAnswer,
  ContentEngineAnswerSource,
  ContentEngineEntry,
  ContentEngineOptions,
} from './plugins/content-engine.js'
export { openApi } from './plugins/openapi.js'
export type { OpenApiMethod, OpenApiOperation, OpenApiOptions } from './plugins/openapi.js'
export { alias, bundleBudget, fonts, requireEnv } from './plugins/build.js'
export type { BundleBudgetOptions, FontsOptions } from './plugins/build.js'
