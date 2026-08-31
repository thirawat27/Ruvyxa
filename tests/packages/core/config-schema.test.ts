import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { describe, it } from 'node:test'
import { pathToFileURL } from 'node:url'

import type { RuvyxaConfig } from '../../../packages/@ruvyxa/core/dist/types.js'
import { repoPath } from '../../repo-root.ts'

/**
 * `ruvyxa.config` has three descriptions of the same object, and they drifted.
 *
 * `ProjectConfig` in `crates/ruvyxa_cli/src/config.rs` decides what the compiler
 * reads; `CONFIG_KEY_SCHEMA` in `packages/ruvyxa/runtime/config-schema.mjs`
 * decides what the build accepts; `RuvyxaConfig` in `@ruvyxa/core` decides what
 * TypeScript accepts. `build.target` lived in the second for several releases
 * and never reached the third, so a project that set it was refused by `tsc`
 * against a build that honoured the value, applied it in both compilers, and
 * documented it.
 *
 * The literal below is annotated `RuvyxaConfig`, so a key the type does not
 * declare fails compilation, and its key set is compared against the schema, so
 * a key the schema declares and the literal omits fails at run time. Neither
 * side can grow a key alone.
 *
 * That pair alone is not enough, which is how `image.maxWidth` was lost: Rust
 * declared it, the public type declared it, and both of the descriptions this
 * file compared agreed with each other in leaving it out — so every command
 * refused a config that set the documented option. The third edge is
 * `tests/fixtures/config-surface-conformance.json`, generated from serde by
 * `config_surface_matches_the_rust_config` in `crates/ruvyxa_cli/src/tests.rs`
 * and replayed against the schema below.
 */
const { CONFIG_KEY_SCHEMA, DEPRECATED_CONFIG_KEYS } = (await import(
  pathToFileURL(repoPath('packages/ruvyxa/runtime/config-schema.mjs')).href
)) as {
  CONFIG_KEY_SCHEMA: Readonly<Record<string, readonly string[]>>
  DEPRECATED_CONFIG_KEYS: readonly string[]
}

/**
 * The field set of the Rust config structs, read back out of serde.
 *
 * Nothing in it is transcribed by hand: every struct behind it carries
 * `deny_unknown_fields`, so it names the fields it accepts in the error it
 * raises for one it does not, and the Rust test regenerates this table from
 * those errors. `react` and `typescript` are present here as well as in the
 * schema — `ProjectConfig` accepts both so an older config still validates —
 * so `DEPRECATED_CONFIG_KEYS` plays no part in this comparison.
 */
const configSurface = JSON.parse(
  readFileSync(repoPath('tests/fixtures/config-surface-conformance.json'), 'utf8'),
) as {
  sections: Record<string, { fields: string[] }>
}

/** Code-unit order, which is what both replays and the fixture are written in. */
function sorted(values: readonly string[]): string[] {
  return values.slice().sort()
}

/**
 * Every option `ruvyxa.config.ts` accepts, written as a project would write it.
 *
 * Values are only there to make the literal well-typed; the keys are the
 * contract. `react` and `typescript` are absent on purpose — see
 * `DEPRECATED_CONFIG_KEYS`.
 */
const authored: RuvyxaConfig = {
  appDir: 'app',
  outDir: '.ruvyxa',
  runtime: 'node',
  reactCompiler: false,
  typedRoutes: true,
  css: { entries: ['styles/global.css'] },
  markdown: {
    gfm: true,
    remarkPlugins: [],
    rehypePlugins: [],
    recmaPlugins: [],
    remarkRehypeOptions: {},
  },
  server: { host: 'localhost', port: 3000 },
  build: {
    minify: true,
    map: false,
    treeShake: true,
    split: 'route',
    workers: 4,
    jsx: 'automatic',
    target: 'es2022',
    manifest: false,
    warm: true,
    prerenderCache: true,
  },
  render: { strategy: 'ssr', revalidate: 60 },
  debug: { overlay: true, traces: false },
  image: {
    optimize: true,
    quality: 82,
    lossless: false,
    keepOriginal: false,
    variantWidths: [640, 1280],
    maxWidth: 3840,
    workers: 0,
    effort: 4,
    onDemand: { enabled: true, maxWidth: 3840 },
  },
  i18n: {
    locales: ['en', 'th'],
    defaultLocale: 'en',
    localeParam: 'lang',
    detectLocale: true,
    cookie: 'RUVYXA_LOCALE',
  },
  security: {
    actionLimit: 1048576,
    apiLimit: 10485760,
    pluginLimit: 33554432,
    actionRateLimit: { max: 600, window: 60 },
    sameOrigin: true,
    fetchMeta: true,
    trustedProxyIps: ['10.0.0.0/8'],
    headers: true,
  },
  cache: {
    routes: true,
    css: true,
    dir: '.cache',
    handler: './cache-handler.mjs',
    maxEntries: 1024,
  },
  site: {
    url: 'https://example.com',
    title: 'Example',
    description: 'An example site',
    language: 'en',
    sitemap: {
      exclude: ['/draft/*'],
      additionalPaths: ['/legacy'],
      defaults: { lastModified: '2026-01-01', changeFrequency: 'weekly', priority: 0.5 },
      entries: [
        {
          url: '/',
          lastModified: '2026-01-01',
          changeFrequency: 'daily',
          priority: 1,
          alternates: { languages: { th: 'https://example.com/th' } },
          images: ['https://example.com/hero.png'],
          videos: [
            {
              title: 'Intro',
              thumbnail_loc: 'https://example.com/thumb.jpg',
              description: 'An intro video',
              content_loc: 'https://example.com/intro.mp4',
              player_loc: 'https://example.com/player',
              duration: 120,
              view_count: 10,
              rating: 4.5,
              expiration_date: '2027-01-01',
              publication_date: '2026-01-01',
              family_friendly: 'yes',
              requires_subscription: 'no',
              live: 'no',
              restriction: { relationship: 'allow', content: 'TH' },
              platform: { relationship: 'allow', content: 'web' },
              uploader: { content: 'Example', info: 'https://example.com/about' },
              tag: ['intro'],
            },
          ],
        },
      ],
    },
    robots: {
      rules: [{ userAgent: '*', allow: ['/'], disallow: ['/admin'], crawlDelay: 1 }],
      sitemap: ['https://example.com/sitemap.xml'],
      host: 'example.com',
    },
  },
  content: {
    engine: {
      exclude: ['/draft/*'],
      locale: 'en',
      stopWords: ['the'],
      minTermLength: 2,
      manifestPath: '/content.json',
      searchPath: '/search-index.json',
      feedPath: '/rss.xml',
      sitemapPath: '/sitemap.xml',
      llmsPath: '/llms.txt',
      language: 'en',
    },
  },
  middleware: {
    workers: 2,
    timeoutMs: 5000,
    builtin: {
      cors: {
        origins: ['https://example.com'],
        methods: ['GET'],
        headers: ['content-type'],
        credentials: false,
        maxAge: 600,
      },
      timing: true,
      log: true,
      rate: { max: 100, window: 60, key: 'ip' },
      headers: { 'x-example': '1' },
    },
  },
  // An inventory of every key, not a runnable config: the build refuses
  // `adapter` and `adapterOptions` together, and nothing here is ever loaded.
  adapter: {
    name: 'fixture',
    target: 'node',
    supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
    build: () => ({ name: 'fixture', target: 'node', entry: 'entry', assetsDir: 'assets' }),
  },
  adapterOptions: { serviceName: 'example' },
  plugins: [],
}

/**
 * Read the object the schema path names out of the authored literal.
 *
 * `[]` in a path means "one element of this array", which is how the schema
 * describes a repeated shape once. The first element is the one checked,
 * because the literal carries exactly one of each.
 */
function sectionAt(schemaPath: string): Record<string, unknown> | undefined {
  let current: unknown = { config: authored }
  for (const rawSegment of schemaPath.split('.')) {
    const isElement = rawSegment.endsWith('[]')
    const segment = isElement ? rawSegment.slice(0, -2) : rawSegment
    if (current === null || typeof current !== 'object') return undefined
    current = (current as Record<string, unknown>)[segment]
    if (isElement) {
      if (!Array.isArray(current)) return undefined
      current = current[0]
    }
  }
  return current !== null && typeof current === 'object' && !Array.isArray(current)
    ? (current as Record<string, unknown>)
    : undefined
}

describe('config schema', () => {
  it('declares every key the type declares, and nothing more', () => {
    const deprecated = new Set(DEPRECATED_CONFIG_KEYS)

    for (const [schemaPath, keys] of Object.entries(CONFIG_KEY_SCHEMA)) {
      const section = sectionAt(schemaPath)
      assert.ok(
        section,
        `${schemaPath} is in the schema but the authored literal has no value there. ` +
          'Add it to the literal, or remove the schema entry.',
      )

      const expected = keys
        .filter((key) => !deprecated.has(key))
        .slice()
        .sort()
      const actual = Object.keys(section).slice().sort()
      assert.deepEqual(
        actual,
        expected,
        `${schemaPath} disagrees between config-schema.mjs and RuvyxaConfig`,
      )
    }
  })

  // The two keys the schema keeps and the type deliberately drops. Without this
  // the exclusion above could quietly grow to cover a real divergence.
  it('keeps the deprecated keys accepted and undeclared', () => {
    assert.deepEqual(DEPRECATED_CONFIG_KEYS.slice().sort(), ['react', 'typescript'])
    for (const key of DEPRECATED_CONFIG_KEYS) {
      assert.ok(
        CONFIG_KEY_SCHEMA.config.includes(key),
        `${key} must stay accepted so an older config still validates`,
      )
    }
  })

  /**
   * The edge that was missing while `image.maxWidth` was unusable.
   *
   * Both directions, deliberately. A key in Rust and not in the schema is that
   * defect: the compiler reads the option, the renderer refuses the config, and
   * every command fails before it starts. A key in the schema and not in Rust
   * is the quieter one: the config validates, the option is dropped by the
   * projection or ignored by the compiler, and the project never learns that
   * the setting selects no behaviour.
   */
  it('declares every key the Rust config declares, and nothing more', () => {
    assert.deepEqual(
      sorted(Object.keys(CONFIG_KEY_SCHEMA)),
      sorted(Object.keys(configSurface.sections)),
      'config-schema.mjs and tests/fixtures/config-surface-conformance.json describe ' +
        'different sets of config sections',
    )

    for (const [schemaPath, section] of Object.entries(configSurface.sections)) {
      assert.deepEqual(
        sorted(CONFIG_KEY_SCHEMA[schemaPath] ?? []),
        sorted(section.fields),
        `${schemaPath} disagrees between config-schema.mjs and the Rust config. ` +
          'A field only Rust has is refused by RUV1602 before any command runs; ' +
          'a field only the schema has is accepted and then ignored.',
      )
    }
  })

  // A schema entry nothing walks accepts every key silently, which is the
  // failure the whole table exists to prevent.
  it('is reachable from the config root at every path', () => {
    for (const schemaPath of Object.keys(CONFIG_KEY_SCHEMA)) {
      if (schemaPath === 'config') continue
      const parent = schemaPath.slice(0, schemaPath.lastIndexOf('.'))
      const leaf = schemaPath.slice(schemaPath.lastIndexOf('.') + 1).replace(/\[\]$/, '')
      assert.ok(
        CONFIG_KEY_SCHEMA[parent]?.includes(leaf),
        `${schemaPath} has no parent entry naming ${leaf}`,
      )
    }
  })
})
