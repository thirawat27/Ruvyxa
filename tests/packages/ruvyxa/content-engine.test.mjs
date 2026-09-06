import assert from 'node:assert/strict'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { after, describe, it } from 'node:test'

import {
  contentEngine,
  createContentEngine,
} from '../../../packages/ruvyxa/dist/content-engine/index.js'

const tempDirs = []
after(() => {
  for (const dir of tempDirs) rmSync(dir, { recursive: true, force: true })
})

function tempDir(prefix) {
  const dir = mkdtempSync(path.join(tmpdir(), prefix))
  tempDirs.push(dir)
  return dir
}

function contentProject() {
  const root = tempDir('ruvyxa-content-engine-')
  const writePage = (relative, source) => {
    const file = path.join(root, 'app', relative)
    mkdirSync(path.dirname(file), { recursive: true })
    writeFileSync(file, source)
  }
  writePage(
    '(marketing)/blog/launch/page.mdx',
    `---
title: Launch Day
description: The fast Ruvyxa launch.
publishedAt: 2026-07-22
updatedAt: 2026-07-23T10:30:00Z
author: Ada
tags: [release, framework]
answers:
  - question: Does Ruvyxa support citeable answers?
    answer: Yes. Answer data is explicit and links back to the canonical page.
    sources:
      - name: Ruvyxa rendering guide
        url: /docs/rendering
campaign:
  featured: true
---
# {frontmatter.title}

Ruvyxa ships **fast content** for everyone.
`,
  )
  writePage('about/page.md', '# About Ruvyxa\n\nA framework built for clear delivery.')
  writePage('blog/draft/page.md', '---\ndraft: true\n---\n# Secret roadmap')
  writePage('_private/page.md', '# Private notes')
  writePage('[slug]/page.md', '# Dynamic content')
  return root
}

const options = {
  siteUrl: 'https://example.com',
  title: 'Example content',
  description: 'News from Example.',
  locale: 'en',
}

describe('content engine', () => {
  it('derives live content, search, RSS, sitemap, and llms.txt artifacts from one source', () => {
    const root = contentProject()
    const engine = contentEngine(options)
    assert.deepEqual(
      [...engine.paths],
      ['/content.json', '/search-index.json', '/rss.xml', '/sitemap.xml', '/llms.txt'],
    )
    assert.equal(engine.warning, undefined)

    const manifestBody = engine.artifact(root, '/content.json').body
    const manifest = JSON.parse(manifestBody)
    assert.deepEqual(
      manifest.entries.map((entry) => entry.route),
      ['/blog/launch', '/about'],
    )
    assert.equal(manifest.entries[0].url, 'https://example.com/blog/launch')
    assert.equal(manifest.entries[0].publishedAt, '2026-07-22T00:00:00.000Z')
    assert.equal(manifest.entries[0].frontmatter.campaign.featured, true)
    assert.deepEqual(manifest.entries[0].tags, ['framework', 'release'])
    assert.deepEqual(manifest.entries[0].answers, [
      {
        question: 'Does Ruvyxa support citeable answers?',
        answer: 'Yes. Answer data is explicit and links back to the canonical page.',
        sources: [{ name: 'Ruvyxa rendering guide', url: 'https://example.com/docs/rendering' }],
      },
    ])
    assert.equal(manifest.entries[1].title, 'About Ruvyxa')
    assert.equal(
      manifest.entries[1].description,
      'About Ruvyxa A framework built for clear delivery.',
    )
    assert.equal(
      manifest.entries.some((entry) => entry.route.includes('draft')),
      false,
    )

    const search = engine.artifact(root, '/search-index.json')
    assert.equal(search.contentType, 'application/json; charset=utf-8')
    const searchIndex = JSON.parse(search.body)
    assert.deepEqual(searchIndex.terms.framework, ['/about', '/blog/launch'])
    assert.deepEqual(searchIndex.terms.content, ['/blog/launch'])

    const feed = engine.artifact(root, '/rss.xml')
    assert.equal(feed.contentType, 'application/rss+xml; charset=utf-8')
    assert.match(feed.body, /<title>Launch Day<\/title>/)
    assert.match(feed.body, /<author>Ada<\/author>/)
    assert.doesNotMatch(feed.body, /Secret roadmap/)

    const sitemap = engine.artifact(root, '/sitemap.xml').body
    assert.match(sitemap, /<loc>https:\/\/example\.com\/blog\/launch<\/loc>/)
    assert.match(sitemap, /<lastmod>2026-07-23T10:30:00\.000Z<\/lastmod>/)
    assert.doesNotMatch(sitemap, /\[slug\]|_private|draft/)

    const llms = engine.artifact(root, '/llms.txt')
    assert.equal(llms.contentType, 'text/plain; charset=utf-8')
    assert.match(llms.body, /^# Example content\n\n> News from Example\./)
    assert.match(
      llms.body,
      /\[Launch Day\]\(<https:\/\/example\.com\/blog\/launch>\): The fast Ruvyxa launch\./,
    )
    assert.match(llms.body, /Does Ruvyxa support citeable answers\? — Yes\./)

    // The build writes the same bytes the live path answers with.
    const outDir = tempDir('ruvyxa-content-engine-out-')
    engine.write(root, outDir)
    for (const [name, expected] of [
      ['content.json', manifestBody],
      ['search-index.json', search.body],
      ['rss.xml', feed.body],
      ['sitemap.xml', sitemap],
      ['llms.txt', llms.body],
    ]) {
      assert.equal(readFileSync(path.join(outDir, 'assets', name), 'utf8'), expected)
    }
  })

  it('answers nothing for paths it does not generate or a missing source tree', () => {
    const root = contentProject()
    const engine = contentEngine(options)
    assert.equal(engine.artifact(root, '/other.json'), undefined)
    assert.equal(engine.artifact(path.join(root, 'missing'), '/content.json'), undefined)
  })

  it('re-derives live artifacts when a content page changes', () => {
    const root = contentProject()
    const engine = contentEngine(options)
    assert.match(
      engine.artifact(root, '/content.json').body,
      /A framework built for clear delivery/,
    )
    writeFileSync(
      path.join(root, 'app', 'about', 'page.md'),
      '# About Ruvyxa\n\nUpdated content appears without restarting the development server.',
    )
    assert.match(
      engine.artifact(root, '/content.json').body,
      /Updated content appears without restarting/,
    )
  })

  it('rejects unsafe configuration and invalid content metadata', () => {
    assert.throws(() => contentEngine({ ...options, appDir: '../content' }), /project root/)
    assert.throws(
      () => contentEngine({ ...options, feedPath: '/same', sitemapPath: '/same' }),
      /must be distinct/,
    )
    assert.throws(() => contentEngine({ ...options, locale: 'invalid_locale' }), /BCP 47/)

    const root = tempDir('ruvyxa-content-engine-invalid-')
    const outDir = tempDir('ruvyxa-content-engine-invalid-out-')
    const file = path.join(root, 'app', 'bad', 'page.md')
    mkdirSync(path.dirname(file), { recursive: true })
    const engine = contentEngine(options)

    writeFileSync(file, '---\ntags: release\n---\n# Bad metadata')
    assert.throws(() => engine.write(root, outDir), /frontmatter\.tags/)

    writeFileSync(file, '---\npublishedAt: 2026-02-31\n---\n# Invalid date')
    assert.throws(() => engine.write(root, outDir), /ISO date string/)

    writeFileSync(file, '---\nnull\n---\n# Invalid mapping')
    assert.throws(() => engine.write(root, outDir), /YAML mapping/)

    writeFileSync(file, '---\nanswers:\n  - question: Missing answer\n---\n# Invalid answer')
    assert.throws(() => engine.write(root, outDir), /answers\[0\]\.answer/)

    writeFileSync(
      file,
      '---\nanswers:\n  - question: Bad source\n    answer: Explicit\n    sources:\n      - name: Local\n        url: javascript:alert(1)\n---\n# Invalid source',
    )
    assert.throws(() => engine.write(root, outDir), /must use http\(s\)/)
  })

  it('escapes markdown syntax in llms.txt titles and descriptions', () => {
    const root = tempDir('ruvyxa-content-engine-')
    const file = path.join(root, 'app', 'notes', 'page.md')
    mkdirSync(path.dirname(file), { recursive: true })
    writeFileSync(
      file,
      "---\ntitle: 'Notes [draft]'\ndescription: 'See [the guide](/docs) for C:\\paths.'\n---\n# Notes\n",
    )
    const body = contentEngine(options).artifact(root, '/llms.txt').body
    assert.ok(
      body.includes(
        '- [Notes \\[draft\\]](<https://example.com/notes>): See \\[the guide\\](/docs) for C:\\\\paths.',
      ),
      `llms.txt entry was not escaped: ${body}`,
    )
  })

  it('can disable the llms.txt artifact', () => {
    const root = contentProject()
    const engine = contentEngine({ ...options, llmsPath: false })
    assert.doesNotMatch(engine.paths.join(','), /llms\.txt/)
    assert.equal(engine.artifact(root, '/llms.txt'), undefined)
    const outDir = tempDir('ruvyxa-content-engine-out-')
    engine.write(root, outDir)
    assert.equal(existsSync(path.join(outDir, 'assets', 'llms.txt')), false)
  })

  it('warns once, by code, when the project names no locale', () => {
    const { locale: _locale, ...unlocalized } = options
    assert.match(contentEngine(unlocalized).warning, /^RUV2207 /)
  })

  it('is built from the config, with site identity from the shared site block', () => {
    const site = {
      url: 'https://example.com',
      title: 'Example',
      description: 'News',
      language: 'en',
    }
    assert.equal(createContentEngine({ site }), undefined)
    assert.equal(createContentEngine({ site, content: false }), undefined)
    assert.equal(createContentEngine({ site, content: { engine: false } }), undefined)
    assert.deepEqual(
      [...createContentEngine({ site, content: true }).paths],
      ['/content.json', '/search-index.json', '/rss.xml', '/sitemap.xml', '/llms.txt'],
    )
    assert.deepEqual(
      [...createContentEngine({ site, content: { engine: { llmsPath: false } } }).paths],
      ['/content.json', '/search-index.json', '/rss.xml', '/sitemap.xml'],
    )
    assert.throws(
      () => createContentEngine({ site: { url: site.url }, content: true }),
      /site\.title must be a non-empty string/,
    )
  })
})
