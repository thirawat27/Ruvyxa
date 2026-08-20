import type { SiteSitemapEntry, SiteSitemapEntryDefaults, SiteSitemapVideo } from '@ruvyxa/core'

import {
  compareStable,
  escapeXml,
  isConcreteApplicationPath,
  stringList,
  validateAbsoluteHttpUrl,
} from './shared.js'

const SITEMAP_MAX_URLS = 50_000
const SITEMAP_MAX_BYTES = 50 * 1024 * 1024
const SITEMAP_FOOTER = '</urlset>\n'

interface ResolvedPluginSitemapEntry extends Omit<SiteSitemapEntry, 'url' | 'lastModified'> {
  location: string
  lastModified?: string
}

export function pluginSitemapEntries(
  paths: string[],
  siteUrl: string,
  defaults: SiteSitemapEntryDefaults = {},
  configuredEntries: SiteSitemapEntry[] = [],
): ResolvedPluginSitemapEntry[] {
  const normalizedDefaults = normalizePluginSitemapMetadata(defaults, 'sitemap.defaults')
  const entries = new Map<string, ResolvedPluginSitemapEntry>()
  for (const routePath of paths) {
    const location = pluginSitemapLocation(routePath, siteUrl, 'sitemap route')
    entries.set(location, { location, ...normalizedDefaults })
  }
  for (const [index, configured] of configuredEntries.entries()) {
    const field = `sitemap.entries[${index}]`
    if (!configured || typeof configured !== 'object') {
      throw new TypeError(`${field} must be an object`)
    }
    const location = pluginSitemapLocation(configured.url, siteUrl, `${field}.url`)
    const current = entries.get(location) ?? { location, ...normalizedDefaults }
    const metadata = normalizePluginSitemapMetadata(configured, field)
    const alternates = configured.alternates?.languages ?? {}
    for (const [language, href] of Object.entries(alternates)) {
      if (!/^[A-Za-z0-9-]+$/.test(language)) {
        throw new TypeError(`${field}.alternates.languages contains an invalid language tag`)
      }
      validateAbsoluteHttpUrl(href, `${field}.alternates.languages.${language}`)
    }
    const images = configured.images ?? []
    for (const [imageIndex, image] of images.entries()) {
      validateAbsoluteHttpUrl(image, `${field}.images[${imageIndex}]`)
    }
    const videos = configured.videos ?? []
    videos.forEach((video, videoIndex) =>
      validatePluginSitemapVideo(video, `${field}.videos[${videoIndex}]`),
    )
    entries.set(location, {
      ...current,
      ...metadata,
      location,
      alternates: { languages: { ...alternates } },
      images: [...images],
      videos: videos.map((video) => ({ ...video })),
    })
  }
  return [...entries.values()].sort((left, right) =>
    compareStable(pluginSitemapSortKey(left.location), pluginSitemapSortKey(right.location)),
  )
}

function pluginSitemapSortKey(location: string): string {
  const parsed = new URL(location)
  try {
    return `${decodeURIComponent(parsed.pathname)}${parsed.search}`
  } catch {
    return `${parsed.pathname}${parsed.search}`
  }
}

export function sitemapDocuments(entries: ResolvedPluginSitemapEntry[]): string[] {
  const header = pluginSitemapHeader(entries)
  const documents: string[] = []
  let serializedEntries: string[] = []
  let bytes = Buffer.byteLength(header + SITEMAP_FOOTER)
  for (const entryValue of entries) {
    const entry = pluginSitemapEntryXml(entryValue)
    const entryBytes = Buffer.byteLength(entry)
    if (
      serializedEntries.length > 0 &&
      (serializedEntries.length === SITEMAP_MAX_URLS || bytes + entryBytes > SITEMAP_MAX_BYTES)
    ) {
      documents.push(header + serializedEntries.join('') + SITEMAP_FOOTER)
      serializedEntries = []
      bytes = Buffer.byteLength(header + SITEMAP_FOOTER)
    }
    if (bytes + entryBytes > SITEMAP_MAX_BYTES) {
      throw new TypeError(`sitemap: ${entryValue.location} cannot fit within the 50 MB limit`)
    }
    serializedEntries.push(entry)
    bytes += entryBytes
  }
  documents.push(header + serializedEntries.join('') + SITEMAP_FOOTER)
  return documents
}

function pluginSitemapHeader(entries: ResolvedPluginSitemapEntry[]): string {
  const alternates = entries.some((entry) => Object.keys(entry.alternates?.languages ?? {}).length)
  const images = entries.some((entry) => entry.images?.length)
  const videos = entries.some((entry) => entry.videos?.length)
  return `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"${alternates ? ' xmlns:xhtml="http://www.w3.org/1999/xhtml"' : ''}${images ? ' xmlns:image="http://www.google.com/schemas/sitemap-image/1.1"' : ''}${videos ? ' xmlns:video="http://www.google.com/schemas/sitemap-video/1.1"' : ''}>\n`
}

function pluginSitemapEntryXml(entry: ResolvedPluginSitemapEntry): string {
  let xml = `  <url>\n    <loc>${escapeXml(entry.location)}</loc>\n`
  for (const [language, href] of Object.entries(entry.alternates?.languages ?? {})) {
    xml += `    <xhtml:link rel="alternate" hreflang="${escapeXml(language)}" href="${escapeXml(href)}" />\n`
  }
  for (const image of entry.images ?? []) {
    xml += `    <image:image>\n      <image:loc>${escapeXml(image)}</image:loc>\n    </image:image>\n`
  }
  for (const video of entry.videos ?? []) xml += pluginSitemapVideoXml(video)
  if (entry.lastModified) xml += `    <lastmod>${escapeXml(entry.lastModified)}</lastmod>\n`
  if (entry.changeFrequency) {
    xml += `    <changefreq>${entry.changeFrequency}</changefreq>\n`
  }
  if (entry.priority !== undefined) xml += `    <priority>${entry.priority}</priority>\n`
  return xml + '  </url>\n'
}

function pluginSitemapVideoXml(video: SiteSitemapVideo): string {
  let xml = '    <video:video>\n'
  const element = (name: string, value: string | number | undefined) => {
    if (value !== undefined)
      xml += `      <video:${name}>${escapeXml(String(value))}</video:${name}>\n`
  }
  element('title', video.title)
  element('thumbnail_loc', video.thumbnail_loc)
  element('description', video.description)
  element('content_loc', video.content_loc)
  element('player_loc', video.player_loc)
  element('duration', video.duration)
  element('view_count', video.view_count)
  element('rating', video.rating)
  element('expiration_date', normalizeOptionalDate(video.expiration_date, 'video.expiration_date'))
  element(
    'publication_date',
    normalizeOptionalDate(video.publication_date, 'video.publication_date'),
  )
  element('family_friendly', video.family_friendly)
  element('requires_subscription', video.requires_subscription)
  element('live', video.live)
  for (const [name, value] of [
    ['restriction', video.restriction],
    ['platform', video.platform],
  ] as const) {
    if (value) {
      xml += `      <video:${name} relationship="${value.relationship}">${escapeXml(value.content)}</video:${name}>\n`
    }
  }
  if (video.uploader) {
    const info = video.uploader.info ? ` info="${escapeXml(video.uploader.info)}"` : ''
    xml += `      <video:uploader${info}>${escapeXml(video.uploader.content)}</video:uploader>\n`
  }
  for (const tag of stringList(video.tag, 'video.tag')) element('tag', tag)
  return xml + '    </video:video>\n'
}

function normalizePluginSitemapMetadata(
  value: SiteSitemapEntryDefaults,
  field: string,
): SiteSitemapEntryDefaults & { lastModified?: string } {
  const lastModified = normalizeOptionalDate(value.lastModified, `${field}.lastModified`)
  if (
    value.changeFrequency !== undefined &&
    !['always', 'hourly', 'daily', 'weekly', 'monthly', 'yearly', 'never'].includes(
      value.changeFrequency,
    )
  ) {
    throw new TypeError(`${field}.changeFrequency is not supported`)
  }
  if (
    value.priority !== undefined &&
    (!Number.isFinite(value.priority) || value.priority < 0 || value.priority > 1)
  ) {
    throw new TypeError(`${field}.priority must be between 0 and 1`)
  }
  return {
    ...(lastModified ? { lastModified } : {}),
    ...(value.changeFrequency ? { changeFrequency: value.changeFrequency } : {}),
    ...(value.priority !== undefined ? { priority: value.priority } : {}),
  }
}

function normalizeOptionalDate(
  value: string | Date | undefined,
  field: string,
): string | undefined {
  if (value === undefined) return undefined
  if (value instanceof Date) {
    if (!Number.isFinite(value.getTime())) throw new TypeError(`${field} must be a valid ISO date`)
    return value.toISOString()
  }
  if (typeof value !== 'string') throw new TypeError(`${field} must be a valid ISO date`)
  if (/^\d{4}-\d{2}-\d{2}$/.test(value)) {
    const date = new Date(`${value}T00:00:00.000Z`)
    if (Number.isFinite(date.getTime()) && date.toISOString().startsWith(value)) return value
    throw new TypeError(`${field} must be a valid ISO date`)
  }
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(value)) {
    throw new TypeError(`${field} must be a valid ISO date`)
  }
  if (!Number.isFinite(Date.parse(value))) throw new TypeError(`${field} must be a valid ISO date`)
  return value
}

function pluginSitemapLocation(value: string, siteUrl: string, field: string): string {
  if (typeof value !== 'string' || value === '') throw new TypeError(`${field} must be a URL`)
  let location: string
  if (value.startsWith('/')) {
    if (!isConcreteApplicationPath(value)) throw new TypeError(`${field} must be a concrete /path`)
    location = siteUrl + value.split('/').map(encodeURIComponent).join('/')
  } else {
    validateAbsoluteHttpUrl(value, field)
    const parsed = new URL(value)
    if (parsed.origin !== siteUrl) throw new TypeError(`${field} must use origin ${siteUrl}`)
    location = parsed.href === `${siteUrl}/` ? `${siteUrl}/` : parsed.href
  }
  if ([...location].length > 2_048) throw new TypeError(`${field} exceeds 2048 characters`)
  return location
}

/** The three fields Google requires on every sitemap video, plus their URLs. */
function validateSitemapVideoRequired(video: SiteSitemapVideo, field: string): void {
  for (const key of ['title', 'thumbnail_loc', 'description'] as const) {
    if (typeof video[key] !== 'string' || video[key].trim() === '') {
      throw new TypeError(`${field}.${key} must be a non-empty string`)
    }
  }
  validateAbsoluteHttpUrl(video.thumbnail_loc, `${field}.thumbnail_loc`)
  if (video.content_loc) validateAbsoluteHttpUrl(video.content_loc, `${field}.content_loc`)
  if (video.player_loc) validateAbsoluteHttpUrl(video.player_loc, `${field}.player_loc`)
}

/** Numeric bounds the sitemap video schema defines. */
function validateSitemapVideoNumbers(video: SiteSitemapVideo, field: string): void {
  if (
    video.duration !== undefined &&
    (!Number.isInteger(video.duration) || video.duration < 1 || video.duration > 28_800)
  ) {
    throw new TypeError(`${field}.duration must be an integer from 1 to 28800`)
  }
  if (
    video.rating !== undefined &&
    (!Number.isFinite(video.rating) || video.rating < 0 || video.rating > 5)
  ) {
    throw new TypeError(`${field}.rating must be between 0 and 5`)
  }
  if (
    video.view_count !== undefined &&
    (!Number.isInteger(video.view_count) || video.view_count < 0)
  ) {
    throw new TypeError(`${field}.view_count must be a non-negative integer`)
  }
}

/**
 * The yes/no flags, the allow/deny pairs, and the uploader.
 *
 * These are the fields the schema spells as literal strings rather than
 * booleans, so a `true` here would serialize to something no crawler accepts.
 */
function validateSitemapVideoEnums(video: SiteSitemapVideo, field: string): void {
  for (const key of ['family_friendly', 'requires_subscription', 'live'] as const) {
    if (video[key] !== undefined && video[key] !== 'yes' && video[key] !== 'no') {
      throw new TypeError(`${field}.${key} must be "yes" or "no"`)
    }
  }
  for (const key of ['restriction', 'platform'] as const) {
    const relationship = video[key]
    if (relationship === undefined) continue
    if (
      (relationship.relationship !== 'allow' && relationship.relationship !== 'deny') ||
      typeof relationship.content !== 'string' ||
      relationship.content.trim() === ''
    ) {
      throw new TypeError(`${field}.${key} must contain an allow/deny relationship and content`)
    }
  }
  if (video.uploader !== undefined) {
    if (typeof video.uploader.content !== 'string' || video.uploader.content.trim() === '') {
      throw new TypeError(`${field}.uploader.content must be a non-empty string`)
    }
    if (video.uploader.info) {
      validateAbsoluteHttpUrl(video.uploader.info, `${field}.uploader.info`)
    }
  }
}

function validatePluginSitemapVideo(video: SiteSitemapVideo, field: string): void {
  if (!video || typeof video !== 'object') throw new TypeError(`${field} must be an object`)
  validateSitemapVideoRequired(video, field)
  validateSitemapVideoNumbers(video, field)
  normalizeOptionalDate(video.expiration_date, `${field}.expiration_date`)
  normalizeOptionalDate(video.publication_date, `${field}.publication_date`)
  validateSitemapVideoEnums(video, field)
  stringList(video.tag, `${field}.tag`)
}
