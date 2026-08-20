import { definePlugin } from '@ruvyxa/core/plugin'
import type { SiteSitemapEntry, SiteSitemapEntryDefaults } from '@ruvyxa/core'
import type { PluginBuildContext, RuvyxaPlugin } from '@ruvyxa/core/plugin'

import { matchSource } from './http.js'
import {
  escapeXml,
  isConcreteApplicationPath,
  manifestPagePaths,
  normalizeDate,
  normalizeItemUrl,
  normalizePublicFilePath,
  normalizeSiteUrl,
  stringList,
  uniqueStrings,
  validateAbsoluteHttpUrl,
  validateRobotsAgent,
  validateRobotsPath,
  validateRoutePattern,
  writePublicAsset,
} from './shared.js'
import { pluginSitemapEntries, sitemapDocuments } from './sitemap-xml.js'

// ─── sitemap / robots ─────────────────────────────────────────────────────────

export interface SitemapOptions {
  /** Absolute site origin, e.g. `https://example.com`. Required. */
  siteUrl: string
  /** Route paths or trailing-`*` patterns excluded from the sitemap. */
  exclude?: string[]
  /** Concrete root-relative paths that are not present in the route manifest. */
  additionalPaths?: string[]
  /** Metadata inherited by every discovered and explicit entry. */
  defaults?: SiteSitemapEntryDefaults
  /** Next-style entries that enrich discovered routes or add new URLs. */
  entries?: SiteSitemapEntry[]
  /** Also write a `robots.txt` referencing the sitemap. @default false */
  robots?: boolean
}

/**
 * Generates `sitemap.xml` (and optionally `robots.txt`) into the build's
 * public asset directory after every production build, using the route
 * manifest. Dynamic route patterns and non-page routes are skipped.
 */
/** Whether a route survives `sitemap.exclude`. */
function sitemapRouteIncluded(routePath: string, exclude: readonly string[]): boolean {
  return !exclude.some((pattern) => matchSource(pattern, routePath) !== null)
}

/** One `<sitemap>` index entry pointing at a shard file. */
function sitemapIndexEntry(siteUrl: string, index: number): string {
  const shardUrl = `${siteUrl}/sitemap-${index}.xml`
  return `  <sitemap><loc>${escapeXml(shardUrl)}</loc></sitemap>`
}

/** Write either the single sitemap document, or a shard set plus its index. */
function writeSitemapDocuments(
  context: PluginBuildContext,
  documents: readonly string[],
  siteUrl: string,
): void {
  if (documents.length === 1) {
    writePublicAsset(context, 'sitemap.xml', documents[0])
    return
  }
  documents.forEach((document, index) => {
    writePublicAsset(context, `sitemap-${index}.xml`, document)
  })
  const entries = documents.map((_, index) => sitemapIndexEntry(siteUrl, index)).join('\n')
  writePublicAsset(
    context,
    'sitemap.xml',
    `<?xml version="1.0" encoding="UTF-8"?>\n<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${entries}\n</sitemapindex>\n`,
  )
}

/**
 * Build-complete handler for the `sitemap` plugin.
 *
 * Pulled out of `register()` so the plugin body is a single call rather than a
 * closure nested inside `register({ build })` inside `build.onComplete(...)`:
 * the sitemap-specific logic reads the same either way, but as a named
 * function it no longer counts against how deep `sitemap()` itself nests.
 */
function sitemapOnComplete(
  context: PluginBuildContext,
  options: SitemapOptions,
  siteUrl: string,
  additionalPaths: readonly string[],
  exclude: readonly string[],
): void {
  const paths = uniqueStrings([...manifestPagePaths(context), ...additionalPaths]).filter(
    (routePath) => sitemapRouteIncluded(routePath, exclude),
  )
  const entries = pluginSitemapEntries(paths, siteUrl, options.defaults, options.entries)
  const documents = sitemapDocuments(entries)
  writeSitemapDocuments(context, documents, siteUrl)
  if (options.robots === true) {
    writePublicAsset(
      context,
      'robots.txt',
      `User-agent: *\nAllow: /\n\nSitemap: ${siteUrl}/sitemap.xml\n`,
    )
  }
}

export function sitemap(options: SitemapOptions): RuvyxaPlugin {
  const siteUrl = normalizeSiteUrl(options?.siteUrl, 'sitemap')
  const exclude = options.exclude ?? []
  exclude.forEach((pattern, index) => validateRoutePattern(pattern, `sitemap.exclude[${index}]`))
  const additionalPaths = options.additionalPaths ?? []
  additionalPaths.forEach((routePath, index) => {
    if (!isConcreteApplicationPath(routePath)) {
      throw new TypeError(`sitemap.additionalPaths[${index}] must be a concrete /path`)
    }
  })

  return definePlugin({
    name: 'ruvyxa:sitemap',
    register({ build }) {
      build.onComplete((context) =>
        sitemapOnComplete(context, options, siteUrl, additionalPaths, exclude),
      )
    },
  })
}

export interface RobotsRule {
  /** @default "*" */
  userAgent?: string | string[]
  allow?: string | string[]
  disallow?: string | string[]
  crawlDelay?: number
}

export interface RobotsOptions {
  /** Access rules per user agent. Defaults to allowing everything. */
  rules?: RobotsRule | RobotsRule[]
  /** Separate OpenAI search discovery from model-training access. */
  openAi?: {
    /** Controls OAI-SearchBot. */
    search?: boolean
    /** Controls GPTBot. */
    training?: boolean
  }
  /** Absolute sitemap URL appended as a `Sitemap:` line. */
  sitemap?: string | string[]
  /** Preferred absolute site origin written as a `Host:` record. */
  host?: string
}

/** Generates `robots.txt` into the build's public asset directory. */
export function robots(options: RobotsOptions = {}): RuvyxaPlugin {
  let configuredRules: readonly RobotsRule[] = []
  if (options.rules) {
    configuredRules = Array.isArray(options.rules) ? options.rules : [options.rules]
  }
  const rules: RobotsRule[] = configuredRules.length
    ? configuredRules.map((rule) => ({ ...rule }))
    : [{ userAgent: '*', allow: ['/'] }]
  for (const [field, userAgent] of [
    ['search', 'OAI-SearchBot'],
    ['training', 'GPTBot'],
  ] as const) {
    const access = options.openAi?.[field]
    if (access !== undefined && typeof access !== 'boolean') {
      throw new TypeError(`robots: openAi.${field} must be a boolean`)
    }
    if (access === undefined) continue
    if (
      rules.some((rule) =>
        stringList(rule.userAgent ?? '*', 'robots.rules.userAgent').some(
          (agent) => agent.toLowerCase() === userAgent.toLowerCase(),
        ),
      )
    ) {
      throw new TypeError(`robots: ${userAgent} is configured by both rules and openAi.${field}`)
    }
    rules.push({ userAgent, ...(access ? { allow: ['/'] } : { disallow: ['/'] }) })
  }

  // Built on first use rather than here, so a malformed rule still surfaces at
  // build time for a project that never requests the file in development.
  let cached: string | undefined
  const body = () => (cached ??= createRobotsBody(rules, options))

  return definePlugin({
    name: 'ruvyxa:robots',
    register({ http, build }) {
      http.onRequest({
        match: [ROBOTS_PATH],
        handler({ request }) {
          if (new URL(request.url).pathname !== ROBOTS_PATH) return undefined
          return new Response(body(), {
            headers: { 'content-type': 'text/plain; charset=utf-8' },
          })
        },
      })
      build.onComplete((context) => writePublicAsset(context, 'robots.txt', body()))
    },
  })
}

const ROBOTS_PATH = '/robots.txt'

/**
 * Render `robots.txt`.
 *
 * Split out of `register()` so the same bytes answer a development request and
 * a production build. `sitemap` cannot do this — its entries come from the
 * build manifest, which does not exist while `ruvyxa dev` is running.
 */
function createRobotsBody(rules: readonly RobotsRule[], options: RobotsOptions): string {
  const blocks = rules.flatMap((rule, ruleIndex) => {
    const agents = stringList(rule.userAgent ?? '*', `robots.rules[${ruleIndex}].userAgent`)
    const allow = stringList(rule.allow, `robots.rules[${ruleIndex}].allow`)
    const disallow = stringList(rule.disallow, `robots.rules[${ruleIndex}].disallow`)
    for (const agent of agents) validateRobotsAgent(agent, ruleIndex)
    for (const value of [...allow, ...disallow]) validateRobotsPath(value, ruleIndex)
    if (
      rule.crawlDelay !== undefined &&
      (!Number.isSafeInteger(rule.crawlDelay) || rule.crawlDelay < 0)
    ) {
      throw new TypeError(`robots.rules[${ruleIndex}].crawlDelay must be a non-negative integer`)
    }
    return agents.map((agent) => {
      const lines = [`User-agent: ${agent}`]
      for (const value of allow) lines.push(`Allow: ${value}`)
      for (const value of disallow) lines.push(`Disallow: ${value}`)
      if (rule.crawlDelay !== undefined) lines.push(`Crawl-delay: ${rule.crawlDelay}`)
      return lines.join('\n')
    })
  })
  let body = blocks.join('\n\n') + '\n'
  const sitemaps = stringList(options.sitemap, 'robots.sitemap')
  for (const sitemapUrl of sitemaps) {
    validateAbsoluteHttpUrl(sitemapUrl, 'robots.sitemap')
    body += `\nSitemap: ${sitemapUrl}\n`
  }
  if (options.host) body += `\nHost: ${normalizeSiteUrl(options.host, 'robots.host')}\n`
  return body
}

// ─── feed ────────────────────────────────────────────────────────────────────

export interface FeedItem {
  title: string
  /** Absolute URL or a path resolved against `siteUrl`. */
  url: string
  description?: string
  content?: string
  id?: string
  publishedAt?: string | Date
  author?: string
  categories?: string[]
}

export interface FeedOptions {
  siteUrl: string
  title: string
  description: string
  /** Static items or a build-time loader. */
  items: FeedItem[] | (() => FeedItem[] | Promise<FeedItem[]>)
  /** @default "/rss.xml" */
  path?: string
  language?: string
  copyright?: string
}

/** Generates a deterministic RSS 2.0 feed from explicit content metadata. */
export function feed(options: FeedOptions): RuvyxaPlugin {
  const siteUrl = normalizeSiteUrl(options?.siteUrl, 'feed')
  if (typeof options.title !== 'string' || options.title.trim() === '') {
    throw new TypeError('feed: title must be a non-empty string')
  }
  if (typeof options.description !== 'string' || options.description.trim() === '') {
    throw new TypeError('feed: description must be a non-empty string')
  }
  if (!Array.isArray(options.items) && typeof options.items !== 'function') {
    throw new TypeError('feed: items must be an array or build-time loader')
  }
  const outputPath = normalizePublicFilePath(options.path ?? '/rss.xml', 'feed')

  return definePlugin({
    name: 'ruvyxa:feed',
    register({ environment, http, build }) {
      // A static list is cheap enough to answer from either environment. A
      // loader is project work — a file read, a database query — so it runs per
      // request only in development, where there is no built asset to serve and
      // a stale answer would be worse than the cost. In production the build
      // has already written this file.
      const staticItems = Array.isArray(options.items) ? [...options.items] : undefined
      if (staticItems || environment === 'development') {
        let cached: string | undefined
        http.onRequest({
          match: [outputPath],
          async handler({ request }) {
            if (new URL(request.url).pathname !== outputPath) return undefined
            const body = staticItems
              ? (cached ??= createRssFeed(options, siteUrl, staticItems))
              : createRssFeed(options, siteUrl, await resolveFeedItems(options))
            return new Response(body, {
              headers: { 'content-type': 'application/rss+xml; charset=utf-8' },
            })
          },
        })
      }
      build.onComplete(async (context) => {
        const items = await resolveFeedItems(options)
        writePublicAsset(context, outputPath, createRssFeed(options, siteUrl, items))
      })
    },
  })
}

async function resolveFeedItems(options: FeedOptions): Promise<FeedItem[]> {
  const items = typeof options.items === 'function' ? await options.items() : [...options.items]
  if (!Array.isArray(items)) throw new TypeError('feed: item loader must return an array')
  return items
}

// ─── llmsTxt ─────────────────────────────────────────────────────────────────

export interface LlmsTxtLink {
  title: string
  /** Absolute URL or a path resolved against `siteUrl`. */
  url: string
  /** Short note rendered after the link. */
  notes?: string
}

export interface LlmsTxtSection {
  title: string
  links: LlmsTxtLink[]
}

export interface LlmsTxtOptions {
  /** Absolute site origin, e.g. `https://example.com`. Required. */
  siteUrl: string
  /** Rendered as the document's `# ` heading. Required. */
  title: string
  /** One-line summary rendered as the leading blockquote. */
  summary?: string
  /** Free-form paragraphs rendered after the summary. */
  details?: string
  /** Curated sections rendered before discovered routes. */
  sections?: LlmsTxtSection[]
  /** Route paths or trailing-`*` patterns excluded from the discovered list. */
  exclude?: string[]
  /** Heading for discovered static routes. Pass `false` to omit them. @default "Pages" */
  routes?: false | string
  /** @default "/llms.txt" */
  path?: string
}

/**
 * Generates `/llms.txt`, the plain-text site index language models read.
 *
 * Build-time only, like `sitemap` and for the same reason: the discovered page
 * list comes from the route manifest, which a development server has not
 * written yet. Curated `sections` are rendered whether or not routes are
 * included, so a project that wants full control can set `routes: false`.
 */
export function llmsTxt(options: LlmsTxtOptions): RuvyxaPlugin {
  const siteUrl = normalizeSiteUrl(options?.siteUrl, 'llmsTxt')
  if (typeof options.title !== 'string' || options.title.trim() === '') {
    throw new TypeError('llmsTxt: title must be a non-empty string')
  }
  if (options.routes !== undefined && options.routes !== false) {
    if (typeof options.routes !== 'string' || options.routes.trim() === '') {
      throw new TypeError('llmsTxt: routes must be a non-empty heading or false')
    }
  }
  const exclude = options.exclude ?? []
  exclude.forEach((pattern, index) => validateRoutePattern(pattern, `llmsTxt.exclude[${index}]`))
  const sections = (options.sections ?? []).map((section, index) => {
    if (!section || typeof section.title !== 'string' || section.title.trim() === '') {
      throw new TypeError(`llmsTxt: sections[${index}].title must be a non-empty string`)
    }
    if (!Array.isArray(section.links) || section.links.length === 0) {
      throw new TypeError(`llmsTxt: sections[${index}].links must be a non-empty array`)
    }
    return {
      title: section.title,
      links: section.links.map((link, linkIndex) => {
        const at = `llmsTxt.sections[${index}].links[${linkIndex}]`
        if (!link || typeof link.title !== 'string' || link.title.trim() === '') {
          throw new TypeError(`${at}.title must be a non-empty string`)
        }
        if (typeof link.url !== 'string' || link.url.trim() === '') {
          throw new TypeError(`${at}.url must be a non-empty string`)
        }
        return { ...link, url: normalizeItemUrl(link.url, siteUrl, `${at}.url`) }
      }),
    }
  })
  const outputPath = normalizePublicFilePath(options.path ?? '/llms.txt', 'llmsTxt')

  return definePlugin({
    name: 'ruvyxa:llms-txt',
    register({ build }) {
      build.onComplete((context) => {
        const discovered = manifestPagePaths(context).filter(
          (routePath) => !exclude.some((pattern) => matchSource(pattern, routePath) !== null),
        )
        writePublicAsset(context, outputPath, createLlmsTxt(options, siteUrl, sections, discovered))
      })
    },
  })
}

function createLlmsTxt(
  options: LlmsTxtOptions,
  siteUrl: string,
  sections: readonly LlmsTxtSection[],
  discovered: readonly string[],
): string {
  const blocks = [`# ${options.title}`]
  if (options.summary) blocks.push(`> ${options.summary}`)
  if (options.details) blocks.push(options.details)
  for (const section of sections) {
    blocks.push([`## ${section.title}`, ...renderLinks(section.links)].join('\n'))
  }
  if (options.routes !== false && discovered.length > 0) {
    const heading = typeof options.routes === 'string' ? options.routes : 'Pages'
    const links = discovered.map((routePath) => ({
      title: routePath === '/' ? 'Home' : routePath,
      url: `${siteUrl}${routePath}`,
    }))
    blocks.push([`## ${heading}`, ...renderLinks(links)].join('\n'))
  }
  return `${blocks.join('\n\n')}\n`
}

function renderLinks(links: readonly LlmsTxtLink[]): string[] {
  return links.map((link) => `- [${link.title}](${link.url})${link.notes ? `: ${link.notes}` : ''}`)
}

export function createRssFeed(options: FeedOptions, siteUrl: string, items: FeedItem[]): string {
  const entries = items.map((item, index) => {
    if (!item || typeof item.title !== 'string' || item.title.trim() === '') {
      throw new TypeError(`feed: items[${index}].title must be a non-empty string`)
    }
    if (typeof item.url !== 'string' || item.url.trim() === '') {
      throw new TypeError(`feed: items[${index}].url must be a non-empty string`)
    }
    const url = normalizeItemUrl(item.url, siteUrl, `feed.items[${index}].url`)
    const id = item.id ?? url
    const lines = [
      '    <item>',
      `      <title>${escapeXml(item.title)}</title>`,
      `      <link>${escapeXml(url)}</link>`,
      `      <guid isPermaLink="${item.id ? 'false' : 'true'}">${escapeXml(id)}</guid>`,
    ]
    if (item.description)
      lines.push(`      <description>${escapeXml(item.description)}</description>`)
    if (item.content) {
      lines.push(
        `      <content:encoded><![CDATA[${item.content.replaceAll(']]>', ']]]]><![CDATA[>')}]]></content:encoded>`,
      )
    }
    if (item.publishedAt) {
      const field = `feed.items[${index}]`
      lines.push(`      <pubDate>${normalizeDate(item.publishedAt, field)}</pubDate>`)
    }
    if (item.author) lines.push(`      <author>${escapeXml(item.author)}</author>`)
    for (const category of item.categories ?? []) {
      lines.push(`      <category>${escapeXml(category)}</category>`)
    }
    lines.push('    </item>')
    return lines.join('\n')
  })
  return `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/">
  <channel>
    <title>${escapeXml(options.title)}</title>
    <link>${escapeXml(siteUrl)}</link>
    <description>${escapeXml(options.description)}</description>
${options.language ? `    <language>${escapeXml(options.language)}</language>\n` : ''}${options.copyright ? `    <copyright>${escapeXml(options.copyright)}</copyright>\n` : ''}${entries.join('\n')}
  </channel>
</rss>
`
}
