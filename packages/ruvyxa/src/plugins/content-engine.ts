import path from 'node:path'
import { createHash } from 'node:crypto'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { isMap, isScalar, isSeq, parseDocument } from 'yaml'
import { definePlugin } from '@ruvyxa/core/plugin'
import type { RuvyxaPlugin } from '@ruvyxa/core/plugin'

import { matchSource } from './http.js'
import {
  createSearchIndexBody,
  resolveIndexLocale,
  segmentWords,
  unsetLocaleDiagnostic,
} from './search.js'
import { createRssFeed } from './seo.js'
import {
  compareStable,
  escapeXml,
  isDirectory,
  normalizeItemUrl,
  normalizePublicFilePath,
  normalizeRoutes,
  normalizeSiteUrl,
  uniqueStrings,
  writePublicAsset,
} from './shared.js'

// ─── contentEngine ────────────────────────────────────────────────────────────

export interface ContentEngineAnswerSource {
  name: string
  url: string
}

export interface ContentEngineAnswer {
  question: string
  answer: string
  sources?: ContentEngineAnswerSource[]
}

export interface ContentEngineEntry {
  id: string
  route: string
  url: string
  title: string
  description: string
  tags: string[]
  readingTimeMinutes: number
  /** Explicit, author-written answers suitable for visible answer blocks. */
  answers: ContentEngineAnswer[]
  publishedAt?: string
  updatedAt?: string
  author?: string
  /** Original JSON-compatible frontmatter for application-specific fields. */
  frontmatter: Readonly<Record<string, unknown>>
}

export interface ContentEngineOptions {
  siteUrl: string
  title: string
  description: string
  /** Directory containing file-system routes, relative to the project root. @default "app" */
  appDir?: string
  /** Exact route paths or trailing-`*` patterns omitted from every artifact. */
  exclude?: string[]
  /**
   * BCP 47 locale used for search tokenization and reading-time estimates.
   *
   * Falls back to `site.language`, then to the search plugin's
   * `DEFAULT_INDEX_LOCALE` with an `RUV2207` warning. It never falls back to
   * the build host's locale: word segmentation and case folding both depend on
   * it, so that would make the emitted index differ between two machines
   * building the same source.
   */
  locale?: string
  stopWords?: string[]
  /** Ignore shorter search terms. @default 2 */
  minTermLength?: number
  /** @default "/content.json" */
  manifestPath?: string
  /** @default "/search-index.json" */
  searchPath?: string
  /** @default "/rss.xml" */
  feedPath?: string
  /** @default "/sitemap.xml" */
  sitemapPath?: string
  /** Agent discovery index in llms.txt format. Set false to disable. @default "/llms.txt" */
  llmsPath?: string | false
  language?: string
}

interface ContentEngineProjectConfig {
  appDir?: string
  site?: {
    url?: string
    title?: string
    description?: string
    language?: string
  }
  content?:
    | boolean
    | {
        engine?:
          boolean | Omit<ContentEngineOptions, 'siteUrl' | 'title' | 'description' | 'appDir'>
      }
}

interface ContentEngineDocument extends ContentEngineEntry {
  text: string
}

interface ContentArtifact {
  body: string
  contentType: string
}

interface NormalizedContentEngineOptions {
  siteUrl: string
  title: string
  description: string
  appDir: string
  exclude: string[]
  /** Always concrete — see `resolveIndexLocale`. Never the host's default. */
  locale: string
  /** The locale the project actually configured, for the `RUV2207` check. */
  configuredLocale: string | undefined
  stopWords: ReadonlySet<string>
  minTermLength: number
  manifestPath: string
  searchPath: string
  feedPath: string
  sitemapPath: string
  llmsPath: string | undefined
  language: string | undefined
}

/**
 * Turns native Markdown/MDX routes into one content graph and derives a live
 * content API, search index, RSS feed, and sitemap without duplicate metadata.
 */
export function contentEngine(options: ContentEngineOptions): RuvyxaPlugin {
  const normalized = normalizeContentEngineOptions(options)
  const outputPaths = [
    normalized.manifestPath,
    normalized.searchPath,
    normalized.feedPath,
    normalized.sitemapPath,
    ...(normalized.llmsPath ? [normalized.llmsPath] : []),
  ]
  let developmentCache:
    { root: string; fingerprint: string; artifacts: Map<string, ContentArtifact> } | undefined

  const developmentArtifacts = (root: string): Map<string, ContentArtifact> => {
    const appRoot = path.resolve(root, normalized.appDir)
    const files = contentPageFiles(appRoot)
    const fingerprint = contentFilesFingerprint(files)
    if (developmentCache?.root === root && developmentCache.fingerprint === fingerprint) {
      return developmentCache.artifacts
    }
    const artifacts = createContentEngineArtifacts(root, normalized, files)
    developmentCache = { root, fingerprint, artifacts }
    return artifacts
  }

  const localeDiagnostic = unsetLocaleDiagnostic(
    normalized.configuredLocale,
    'content.engine.locale (or site.language)',
  )

  return definePlugin({
    name: 'ruvyxa:content-engine',
    ...(localeDiagnostic ? { diagnostics: localeDiagnostic } : {}),
    register({ http, build }) {
      http.onRequest({
        match: outputPaths,
        handler({ request, root }) {
          if (request.method !== 'GET' && request.method !== 'HEAD') return undefined
          const appRoot = path.resolve(root, normalized.appDir)
          if (!isDirectory(appRoot)) return undefined
          const artifact = developmentArtifacts(root).get(new URL(request.url).pathname)
          if (!artifact) return undefined
          return new Response(request.method === 'HEAD' ? null : artifact.body, {
            headers: {
              'cache-control': 'no-cache',
              'content-type': artifact.contentType,
            },
          })
        },
      })
      build.onComplete((context) => {
        for (const [outputPath, artifact] of createContentEngineArtifacts(
          context.root,
          normalized,
        )) {
          writePublicAsset(context, outputPath, artifact.body)
        }
      })
    },
  })
}

/**
 * Materialize the built-in content engine declared by the top-level content
 * configuration. Kept as one normal first-party plugin so explicit
 * `contentEngine()` users and the shorthand share every runtime behavior.
 *
 * @internal
 */
export function contentEngineFromConfig(
  config: ContentEngineProjectConfig,
): RuvyxaPlugin | undefined {
  const content = config?.content
  // `content: true` means "use the defaults"; an object may carry an explicit
  // engine; anything else (absent, false, an array) declines the engine.
  let engine
  if (content === true) {
    engine = {}
  } else if (content && typeof content === 'object' && !Array.isArray(content)) {
    engine = content.engine
  }
  if (engine !== true && (typeof engine !== 'object' || engine === null || Array.isArray(engine))) {
    return undefined
  }

  const options = engine === true ? {} : engine
  const site = config.site ?? {}
  return contentEngine({
    ...options,
    siteUrl: requiredConfiguredSiteValue(site.url, 'url'),
    title: requiredConfiguredSiteValue(site.title, 'title'),
    description: requiredConfiguredSiteValue(site.description, 'description'),
    appDir: config.appDir,
    locale: options.locale ?? site.language,
    language: options.language ?? site.language,
  })
}

function requiredConfiguredSiteValue(value: unknown, field: string): string {
  if (typeof value !== 'string' || value.trim() === '') {
    throw new TypeError(
      `content engine: site.${field} must be a non-empty string when content is enabled`,
    )
  }
  return value
}

function normalizeContentEngineOptions(
  options: ContentEngineOptions,
): NormalizedContentEngineOptions {
  const siteUrl = normalizeSiteUrl(options?.siteUrl, 'contentEngine')
  for (const field of ['title', 'description'] as const) {
    if (typeof options[field] !== 'string' || options[field].trim() === '') {
      throw new TypeError(`contentEngine: ${field} must be a non-empty string`)
    }
  }
  const appDir = options.appDir ?? 'app'
  if (
    typeof appDir !== 'string' ||
    appDir.trim() === '' ||
    path.isAbsolute(appDir) ||
    appDir.split(/[\\/]+/).some((segment) => segment === '..')
  ) {
    throw new TypeError('contentEngine: appDir must stay inside the project root')
  }
  const exclude = normalizeRoutes(options.exclude, 'contentEngine') ?? []
  const locale = resolveIndexLocale(options.locale, 'contentEngine')
  if (
    options.stopWords !== undefined &&
    (!Array.isArray(options.stopWords) ||
      options.stopWords.some((word) => typeof word !== 'string'))
  ) {
    throw new TypeError('contentEngine: stopWords must be an array of strings')
  }
  const minTermLength = options.minTermLength ?? 2
  if (!Number.isInteger(minTermLength) || minTermLength < 1 || minTermLength > 64) {
    throw new TypeError('contentEngine: minTermLength must be an integer from 1 to 64')
  }
  const manifestPath = normalizePublicFilePath(
    options.manifestPath ?? '/content.json',
    'contentEngine',
  )
  const searchPath = normalizePublicFilePath(
    options.searchPath ?? '/search-index.json',
    'contentEngine',
  )
  const feedPath = normalizePublicFilePath(options.feedPath ?? '/rss.xml', 'contentEngine')
  const sitemapPath = normalizePublicFilePath(
    options.sitemapPath ?? '/sitemap.xml',
    'contentEngine',
  )
  const llmsPath =
    options.llmsPath === false
      ? undefined
      : normalizePublicFilePath(options.llmsPath ?? '/llms.txt', 'contentEngine')
  const artifactPaths = [manifestPath, searchPath, feedPath, sitemapPath, llmsPath].filter(
    (value): value is string => value !== undefined,
  )
  if (new Set(artifactPaths).size !== artifactPaths.length) {
    throw new TypeError('contentEngine: generated artifact paths must be distinct')
  }

  return {
    siteUrl,
    title: options.title,
    description: options.description,
    appDir,
    exclude,
    locale,
    configuredLocale: options.locale,
    stopWords: new Set(
      // `locale` is resolved above and is always a concrete string, so this
      // folds the same way on every machine. Passing `options.locale` straight
      // through would reach ICU as `undefined` and fold by the build host's.
      // oxlint-disable-next-line eslint/no-restricted-properties
      (options.stopWords ?? []).map((word) => word.toLocaleLowerCase(locale)),
    ),
    minTermLength,
    manifestPath,
    searchPath,
    feedPath,
    sitemapPath,
    llmsPath,
    language: options.language,
  }
}

function createContentEngineArtifacts(
  root: string,
  options: NormalizedContentEngineOptions,
  files?: string[],
): Map<string, ContentArtifact> {
  const appRoot = path.resolve(root, options.appDir)
  if (!isDirectory(appRoot)) {
    throw new TypeError(`contentEngine: app directory was not found at ${appRoot}`)
  }
  const documents = discoverContentEngineDocuments(appRoot, options, files)
  const entries = documents.map(({ text: _text, ...entry }) => entry)
  const manifestBody = `${JSON.stringify({ version: 1, entries }, null, 2)}\n`
  const searchBody = createSearchIndexBody(
    documents.map((document) => ({
      id: document.id,
      title: document.title,
      url: document.route,
      text: document.text,
      tags: document.tags,
    })),
    options.locale,
    options.stopWords,
    options.minTermLength,
  )
  const feedItems = documents.map((document) => ({
    title: document.title,
    url: document.route,
    description: document.description,
    publishedAt: document.publishedAt,
    author: document.author,
    categories: document.tags,
  }))
  const feedBody = createRssFeed(
    {
      siteUrl: options.siteUrl,
      title: options.title,
      description: options.description,
      language: options.language,
      items: feedItems,
    },
    options.siteUrl,
    feedItems,
  )
  const sitemapEntries = documents
    .map((document) => {
      const lastModified = document.updatedAt ?? document.publishedAt
      const lastModifiedTag = lastModified ? `<lastmod>${lastModified}</lastmod>` : ''
      return `  <url><loc>${escapeXml(document.url)}</loc>${lastModifiedTag}</url>`
    })
    .join('\n')
  const sitemapBody = `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${sitemapEntries}\n</urlset>\n`
  const artifacts = new Map<string, ContentArtifact>([
    [options.manifestPath, { body: manifestBody, contentType: 'application/json; charset=utf-8' }],
    [options.searchPath, { body: searchBody, contentType: 'application/json; charset=utf-8' }],
    [options.feedPath, { body: feedBody, contentType: 'application/rss+xml; charset=utf-8' }],
    [options.sitemapPath, { body: sitemapBody, contentType: 'application/xml; charset=utf-8' }],
  ])
  if (options.llmsPath) {
    artifacts.set(options.llmsPath, {
      body: createLlmsText(options, documents),
      contentType: 'text/plain; charset=utf-8',
    })
  }
  return artifacts
}

function createLlmsText(
  options: NormalizedContentEngineOptions,
  documents: ContentEngineDocument[],
): string {
  const lines = [
    `# ${escapeMarkdownText(options.title)}`,
    '',
    `> ${options.description.replace(/\s+/g, ' ').trim()}`,
    '',
    '## Content',
    '',
  ]
  for (const document of documents) {
    lines.push(
      `- [${escapeMarkdownText(document.title)}](<${document.url}>): ${escapeMarkdownText(document.description)}`,
    )
    for (const answer of document.answers) {
      lines.push(
        `  - ${escapeMarkdownText(answer.question)} — ${escapeMarkdownText(answer.answer)}`,
      )
    }
  }
  return `${lines.join('\n')}\n`
}

function escapeMarkdownText(value: string): string {
  return value
    .replaceAll('\\', '\\\\')
    .replace(/([[\]])/g, '\\$1')
    .replace(/\s+/g, ' ')
    .trim()
}

function discoverContentEngineDocuments(
  appRoot: string,
  options: NormalizedContentEngineOptions,
  files = contentPageFiles(appRoot),
): ContentEngineDocument[] {
  const routes = new Set<string>()
  const documents: ContentEngineDocument[] = []
  for (const file of files) {
    const route = contentRouteFromFile(appRoot, file)
    if (
      route.includes('[') ||
      options.exclude.some((pattern) => matchSource(pattern, route) !== null)
    ) {
      continue
    }
    if (routes.has(route)) {
      throw new TypeError(`contentEngine: multiple Markdown/MDX pages resolve to ${route}`)
    }
    routes.add(route)
    const source = readFileSync(file, 'utf8')
    const { frontmatter, body } = parseContentEngineSource(source, file)
    if (frontmatter.draft === true) continue
    if (frontmatter.draft !== undefined && typeof frontmatter.draft !== 'boolean') {
      throw new TypeError(`contentEngine: ${file} frontmatter.draft must be a boolean`)
    }
    const text = markdownToPlainText(body)
    const title = contentString(frontmatter.title, 'title', file) ?? firstMarkdownHeading(body)
    const resolvedTitle = title || contentTitleFromRoute(route, options.title)
    const descriptionValue =
      contentString(frontmatter.description, 'description', file) ??
      contentString(frontmatter.summary, 'summary', file) ??
      text
    const tags = contentTags(frontmatter.tags, file)
    const answers = contentAnswers(frontmatter.answers, file, options.siteUrl)
    const publishedAt = contentDate(
      frontmatter.publishedAt ?? frontmatter.date,
      'publishedAt',
      file,
    )
    const updatedAt = contentDate(frontmatter.updatedAt, 'updatedAt', file)
    const author = contentString(frontmatter.author, 'author', file)
    const searchableText = text || resolvedTitle
    const resolvedDescription = descriptionValue || resolvedTitle
    const wordCount = segmentWords(searchableText, options.locale).length
    documents.push({
      id: route,
      route,
      url: options.siteUrl + route,
      title: resolvedTitle,
      description: truncateContentText(resolvedDescription, 160),
      tags,
      readingTimeMinutes: Math.max(1, Math.ceil(wordCount / 200)),
      answers,
      ...(publishedAt ? { publishedAt } : {}),
      ...(updatedAt ? { updatedAt } : {}),
      ...(author ? { author } : {}),
      frontmatter,
      text: searchableText,
    })
  }
  return documents.sort((left, right) => {
    const byDate = compareStable(right.publishedAt ?? '', left.publishedAt ?? '')
    return byDate || compareStable(left.route, right.route)
  })
}

function contentPageFiles(root: string): string[] {
  const files: string[] = []
  const visit = (directory: string): void => {
    const entries = readdirSync(directory, { withFileTypes: true }).sort((left, right) =>
      compareStable(left.name, right.name),
    )
    for (const entry of entries) {
      if (entry.isDirectory()) {
        if (entry.name.startsWith('_') || entry.name.startsWith('@')) continue
        visit(path.join(directory, entry.name))
      } else if (entry.isFile() && (entry.name === 'page.md' || entry.name === 'page.mdx')) {
        files.push(path.join(directory, entry.name))
      }
    }
  }
  visit(root)
  return files
}

function contentFilesFingerprint(files: string[]): string {
  const fingerprint = createHash('sha256')
  for (const file of files) {
    const metadata = statSync(file)
    fingerprint.update(file)
    fingerprint.update('\0')
    fingerprint.update(String(metadata.size))
    fingerprint.update('\0')
    fingerprint.update(String(metadata.mtimeMs))
    fingerprint.update('\0')
  }
  return fingerprint.digest('hex')
}

function contentRouteFromFile(appRoot: string, file: string): string {
  const relativeDirectory = path.relative(appRoot, path.dirname(file))
  const segments = relativeDirectory
    .split(path.sep)
    .filter(Boolean)
    .filter((segment) => !(segment.startsWith('(') && segment.endsWith(')')))
  return segments.length === 0 ? '/' : `/${segments.join('/')}`
}

function parseContentEngineSource(
  source: string,
  file: string,
): { frontmatter: Record<string, unknown>; body: string } {
  const normalized = source
    .replace(/^\uFEFF/, '')
    .replaceAll('\r\n', '\n')
    .replaceAll('\r', '\n')
  if (!normalized.startsWith('---\n')) return { frontmatter: {}, body: normalized }
  const lines = normalized.split('\n')
  const closing = lines.findIndex((line, index) => index > 0 && /^(---|\.\.\.)\s*$/.test(line))
  if (closing === -1) {
    throw new TypeError(`contentEngine: ${file} frontmatter has no closing delimiter`)
  }
  const frontmatterSource = `${lines.slice(1, closing).join('\n')}\n`
  const body = lines.slice(closing + 1).join('\n')
  if (frontmatterSource.trim() === '') return { frontmatter: {}, body }
  const document = parseDocument(frontmatterSource, { schema: 'core' })
  if (document.errors.length > 0) {
    throw new TypeError(`contentEngine: ${file} invalid YAML: ${document.errors[0].message}`)
  }
  let value: unknown
  try {
    assertContentEngineYamlKeys(document.contents)
    value = document.toJS({ maxAliasCount: 100 }) as unknown
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new TypeError(`contentEngine: ${file} frontmatter must be JSON-compatible: ${detail}`)
  }
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`contentEngine: ${file} frontmatter must be a YAML mapping`)
  }
  let frontmatter: Record<string, unknown>
  try {
    assertContentEngineJsonValue(value, new WeakSet())
    frontmatter = JSON.parse(JSON.stringify(value)) as Record<string, unknown>
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new TypeError(`contentEngine: ${file} frontmatter must be JSON-compatible: ${detail}`)
  }
  return { frontmatter, body }
}

function assertContentEngineYamlKeys(node: unknown): void {
  if (isMap(node)) {
    for (const pair of node.items) {
      if (!isScalar(pair.key) || typeof pair.key.value !== 'string') {
        throw new TypeError('YAML mapping keys must be strings')
      }
      assertContentEngineYamlKeys(pair.value)
    }
  } else if (isSeq(node)) {
    for (const child of node.items) assertContentEngineYamlKeys(child)
  }
}

function assertContentEngineJsonValue(value: unknown, ancestors: WeakSet<object>): void {
  if (typeof value === 'number' && !Number.isFinite(value)) {
    throw new TypeError('non-finite numbers are not supported')
  }
  if (value === null || typeof value !== 'object') return
  if (ancestors.has(value)) throw new TypeError('cyclic YAML aliases are not supported')
  ancestors.add(value)
  for (const child of Array.isArray(value) ? value : Object.values(value)) {
    assertContentEngineJsonValue(child, ancestors)
  }
  ancestors.delete(value)
}

function markdownToPlainText(body: string): string {
  const visible: string[] = []
  let fence: string | undefined
  let esmBlock = false
  for (const line of body.split('\n')) {
    const fenceMatch = /^\s*(```+|~~~+)/.exec(line)
    if (fenceMatch) {
      if (!fence) fence = fenceMatch[1][0]
      else if (fence === fenceMatch[1][0]) fence = undefined
      continue
    }
    if (fence) continue
    if (/^\s*(?:import|export)\b/.test(line)) {
      esmBlock = true
      continue
    }
    if (esmBlock) {
      if (line.trim() === '') esmBlock = false
      continue
    }
    visible.push(markdownInlineText(line))
  }
  return visible.join(' ').replace(/\s+/g, ' ').trim()
}

function markdownInlineText(value: string): string {
  let output = value
    .replace(/<!--[^]*?-->/g, ' ')
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/<[^>]+>/g, ' ')
  for (let iteration = 0; iteration < 4 && /\{[^{}]*\}/.test(output); iteration += 1) {
    output = output.replace(/\{[^{}]*\}/g, ' ')
  }
  return output
    .replace(/^\s{0,3}(?:#{1,6}|>|[-+*]\s|\d+[.)]\s)\s*/g, '')
    .replace(/[`*_~]/g, '')
    .replace(/\s+/g, ' ')
    .trim()
}

function firstMarkdownHeading(body: string): string | undefined {
  for (const line of body.split('\n')) {
    const match = /^\s{0,3}#\s+(.+?)\s*#*\s*$/.exec(line)
    if (match) {
      const heading = markdownInlineText(match[1])
      if (heading) return heading
    }
  }
  return undefined
}

function contentString(value: unknown, field: string, file: string): string | undefined {
  if (value === undefined || value === null) return undefined
  if (typeof value !== 'string' || value.trim() === '') {
    throw new TypeError(`contentEngine: ${file} frontmatter.${field} must be a non-empty string`)
  }
  return value.trim()
}

function contentTags(value: unknown, file: string): string[] {
  if (value === undefined || value === null) return []
  if (!Array.isArray(value) || value.some((tag) => typeof tag !== 'string' || tag.trim() === '')) {
    throw new TypeError(`contentEngine: ${file} frontmatter.tags must be an array of strings`)
  }
  return uniqueStrings(value.map((tag) => tag.trim())).sort(compareStable)
}

function contentAnswers(value: unknown, file: string, siteUrl: string): ContentEngineAnswer[] {
  if (value === undefined || value === null) return []
  if (!Array.isArray(value)) {
    throw new TypeError(`contentEngine: ${file} frontmatter.answers must be an array`)
  }
  return value.map((entry, answerIndex) => {
    const field = `answers[${answerIndex}]`
    if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) {
      throw new TypeError(`contentEngine: ${file} frontmatter.${field} must be an object`)
    }
    const record = entry as Record<string, unknown>
    const question = contentString(record.question, `${field}.question`, file)
    const answer = contentString(record.answer, `${field}.answer`, file)
    if (!question) {
      throw new TypeError(
        `contentEngine: ${file} frontmatter.${field}.question must be a non-empty string`,
      )
    }
    if (!answer) {
      throw new TypeError(
        `contentEngine: ${file} frontmatter.${field}.answer must be a non-empty string`,
      )
    }
    if (record.sources === undefined || record.sources === null) return { question, answer }
    if (!Array.isArray(record.sources)) {
      throw new TypeError(`contentEngine: ${file} frontmatter.${field}.sources must be an array`)
    }
    const sources = record.sources.map((source, sourceIndex) => {
      const sourceField = `${field}.sources[${sourceIndex}]`
      if (source === null || typeof source !== 'object' || Array.isArray(source)) {
        throw new TypeError(`contentEngine: ${file} frontmatter.${sourceField} must be an object`)
      }
      const sourceRecord = source as Record<string, unknown>
      const name = contentString(sourceRecord.name, `${sourceField}.name`, file)
      const url = contentString(sourceRecord.url, `${sourceField}.url`, file)
      if (!name) {
        throw new TypeError(
          `contentEngine: ${file} frontmatter.${sourceField}.name must be a non-empty string`,
        )
      }
      if (!url) {
        throw new TypeError(
          `contentEngine: ${file} frontmatter.${sourceField}.url must be a non-empty string`,
        )
      }
      return {
        name,
        url: normalizeItemUrl(
          url,
          siteUrl,
          `contentEngine: ${file} frontmatter.${sourceField}.url`,
        ),
      }
    })
    return { question, answer, sources }
  })
}

/** `YYYY-MM-DD`, the whole value. */
const CONTENT_DATE_ONLY = /^\d{4}-\d{2}-\d{2}$/

/**
 * `YYYY-MM-DDTHH:MM[:SS[.fraction]]` followed by a required `Z` or `+HH:MM` offset.
 *
 * Split from the date-only form rather than folded into one pattern with an
 * optional time group: the combined expression was dense enough that the
 * "offset is mandatory once a time is present" rule was invisible in it.
 */
const CONTENT_DATE_TIME =
  /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(?::\d{2}(?:\.\d{1,9})?)?(?:Z|[+-]\d{2}:\d{2})$/

function isContentDateString(value: string): boolean {
  return CONTENT_DATE_ONLY.test(value) || CONTENT_DATE_TIME.test(value)
}

function contentDate(value: unknown, field: string, file: string): string | undefined {
  if (value === undefined || value === null) return undefined
  if (typeof value !== 'string' || !isContentDateString(value)) {
    throw new TypeError(`contentEngine: ${file} frontmatter.${field} must be an ISO date string`)
  }
  const [year, month, day] = value.slice(0, 10).split('-').map(Number)
  const daysInMonth = [
    31,
    year % 400 === 0 || (year % 4 === 0 && year % 100 !== 0) ? 29 : 28,
    31,
    30,
    31,
    30,
    31,
    31,
    30,
    31,
    30,
    31,
  ]
  if (month < 1 || month > 12 || day < 1 || day > daysInMonth[month - 1]) {
    throw new TypeError(`contentEngine: ${file} frontmatter.${field} must be an ISO date string`)
  }
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) {
    throw new TypeError(`contentEngine: ${file} frontmatter.${field} must be an ISO date string`)
  }
  return date.toISOString()
}

function contentTitleFromRoute(route: string, siteTitle: string): string {
  if (route === '/') return siteTitle
  const segment = route.split('/').at(-1) ?? siteTitle
  const title = segment.replace(/[-_]+/g, ' ').trim()
  // Locale-independent on purpose: this title is baked into generated content
  // metadata and prerendered HTML, so it has to be a function of the route
  // alone. `toLocaleUpperCase()` answers by the host's ICU locale — on a
  // Turkish host `/india` would title-case to `Indi̇a` instead of `India`, and
  // two machines would build the same project to different bytes.
  return title ? title[0].toUpperCase() + title.slice(1) : siteTitle
}

function truncateContentText(value: string, maximum: number): string {
  const normalized = value.replace(/\s+/g, ' ').trim()
  const characters = [...normalized]
  if (characters.length <= maximum) return normalized
  return `${characters
    .slice(0, maximum - 1)
    .join('')
    .trimEnd()}…`
}
