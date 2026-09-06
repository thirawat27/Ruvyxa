import { escapeXml, normalizeDate, normalizeItemUrl } from './shared.js'

// ─── RSS 2.0 ─────────────────────────────────────────────────────────────────

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

export interface FeedChannel {
  title: string
  description: string
  language?: string
  copyright?: string
}

/** Render a deterministic RSS 2.0 feed from explicit content metadata. */
export function createRssFeed(channel: FeedChannel, siteUrl: string, items: FeedItem[]): string {
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
    <title>${escapeXml(channel.title)}</title>
    <link>${escapeXml(siteUrl)}</link>
    <description>${escapeXml(channel.description)}</description>
${channel.language ? `    <language>${escapeXml(channel.language)}</language>\n` : ''}${channel.copyright ? `    <copyright>${escapeXml(channel.copyright)}</copyright>\n` : ''}${entries.join('\n')}
  </channel>
</rss>
`
}
