import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

import { CONFIG_KEY_SCHEMA } from '../../../packages/ruvyxa/runtime/config-schema.mjs'

const run = promisify(execFile)
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const renderer = path.join(repoRoot, 'packages/ruvyxa/runtime/config-renderer.mjs')

/**
 * Render one `ruvyxa.config.ts` and hand back the config the CLI would read.
 *
 * The config exports a plain object rather than calling `config()`, which is
 * the identity function — so this needs no package resolution and the temporary
 * project can live anywhere.
 */
async function render(source) {
  const root = await mkdtemp(path.join(tmpdir(), 'ruvyxa-config-render-'))
  try {
    await writeFile(path.join(root, 'ruvyxa.config.ts'), source)
    // The renderer reports a refusal on stdout and exits non-zero, so the
    // message is in the rejection rather than in the resolution.
    let stdout
    try {
      ;({ stdout } = await run(process.execPath, [renderer, root], { cwd: repoRoot }))
    } catch (error) {
      stdout = error.stdout ?? ''
    }
    const response = JSON.parse(stdout)
    assert.equal(response.ok, true, `the renderer refused the config: ${stdout}`)
    return response.config
  } finally {
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  }
}

/**
 * Every key `CONFIG_KEY_SCHEMA` declares, with a value the renderer accepts.
 *
 * Written out rather than generated, because the point is to be the one place
 * that says "this is the whole config surface" in values a project could
 * actually write. A key added to the schema and not added here fails the sweep
 * below by name, which is the moment to check that the renderer emits it.
 */
const MAXIMAL = {
  appDir: 'app',
  outDir: '.ruvyxa',
  runtime: 'node',
  react: true,
  reactCompiler: true,
  typescript: { strict: true },
  typedRoutes: true,
  css: { entries: ['styles/global.css'] },
  markdown: {
    gfm: true,
    remarkPlugins: [],
    rehypePlugins: [],
    recmaPlugins: [],
    remarkRehypeOptions: { allowDangerousHtml: true },
  },
  server: { host: '0.0.0.0', port: 3100 },
  build: {
    minify: true,
    map: true,
    treeShake: true,
    split: 'route',
    workers: 2,
    jsx: 'automatic',
    target: 'es2022',
    manifest: true,
    warm: true,
    prerenderCache: true,
  },
  render: { strategy: 'isr', revalidate: 120 },
  debug: { overlay: true, traces: true },
  image: {
    optimize: true,
    quality: 70,
    lossless: false,
    keepOriginal: true,
    variantWidths: [320, 640],
    maxWidth: 1920,
    workers: 2,
    effort: 5,
    onDemand: { enabled: true, maxWidth: 2048 },
  },
  i18n: {
    locales: ['en', 'th'],
    defaultLocale: 'en',
    localeParam: 'lang',
    detectLocale: false,
    cookie: 'locale',
  },
  security: {
    actionLimit: 1024,
    apiLimit: 2048,
    pluginLimit: 4096,
    actionRateLimit: { max: 10, window: 60 },
    sameOrigin: true,
    fetchMeta: true,
    trustedProxyIps: ['10.0.0.1'],
    headers: true,
  },
  cache: {
    routes: true,
    css: true,
    dir: '.cache',
    handler: './h.mjs',
    maxEntries: 8,
    maxBytes: 16,
  },
  site: {
    url: 'https://example.test',
    title: 'Title',
    description: 'Description',
    language: 'en',
    sitemap: {
      exclude: ['/private'],
      additionalPaths: ['/extra'],
      defaults: { lastModified: '2026-01-01', changeFrequency: 'daily', priority: 0.5 },
      entries: [
        {
          url: '/one',
          lastModified: '2026-01-01',
          changeFrequency: 'weekly',
          priority: 0.8,
          alternates: { languages: { th: '/th/one' } },
          images: ['/a.png'],
          videos: [
            {
              title: 'V',
              thumbnail_loc: '/t.png',
              description: 'D',
              content_loc: '/v.mp4',
              player_loc: '/p',
              duration: 10,
              view_count: 5,
              rating: 4.5,
              expiration_date: '2027-01-01',
              publication_date: '2026-01-01',
              family_friendly: 'yes',
              requires_subscription: 'no',
              live: 'no',
              restriction: { relationship: 'allow', content: 'TH' },
              platform: { relationship: 'allow', content: 'web' },
              uploader: { content: 'Someone', info: 'https://example.test/u' },
              tag: ['a'],
            },
          ],
        },
      ],
    },
    robots: {
      rules: [{ userAgent: '*', allow: ['/'], disallow: ['/private'], crawlDelay: 2 }],
      sitemap: ['https://example.test/sitemap.xml'],
      host: 'example.test',
    },
  },
  content: {
    engine: {
      exclude: ['/x'],
      locale: 'en',
      stopWords: ['the'],
      minTermLength: 3,
      manifestPath: '/m.json',
      searchPath: '/s.json',
      feedPath: '/f.xml',
      sitemapPath: '/sm.xml',
      llmsPath: '/llms.txt',
      language: 'en',
    },
  },
  middleware: {
    builtin: {
      cors: {
        origins: ['https://example.test'],
        methods: ['GET'],
        headers: ['x-a'],
        credentials: true,
        maxAge: 60,
      },
      timing: true,
      log: true,
      rate: { max: 5, window: 30, key: 'ip' },
      headers: { 'x-h': 'v' },
    },
    workers: 2,
    timeoutMs: 5000,
  },
  adapterOptions: { some: 'value' },
  plugins: [],
}

/**
 * Keys whose value cannot be written as JSON, spliced into the config source.
 *
 * `adapter` has to be a live object with a `build(context)` function — the
 * renderer refuses anything else — so it cannot travel in the data literal
 * above. It is still a declared key and still has to be emitted, so it is
 * covered here rather than exempted.
 */
const SOURCE_ONLY = {
  adapter: "{ name: 'node', build() { return { name: 'node', target: 'node' } } }",
}

/**
 * Keys the renderer deliberately does not put in the JSON it hands the CLI.
 *
 * Two, and each carries the reason it is not a defect. Anything else that has
 * to be added here is the defect this suite exists for.
 */
const CARRIED_ELSEWHERE = {
  // Deprecated. Accepted so an older config still renders, read by nothing —
  // strictness belongs in the project's own `tsconfig.json`. `react` is the
  // other deprecated key and is emitted, which is harmless; the asymmetry is
  // not worth a second rule.
  config: ['typescript'],
  // The Markdown configuration cannot cross JSON at all: `remarkPlugins` and
  // friends hold functions. `writeRuntimeConfigPointer` emits a module that
  // re-imports the compiled config bundle, and `compiler.mjs` loads the plugins
  // from there. What the JSON carries is a boolean meaning "there is one",
  // which is all the CLI needs to know.
  'config.markdown': [
    'gfm',
    'remarkPlugins',
    'rehypePlugins',
    'recmaPlugins',
    'remarkRehypeOptions',
  ],
}

/** Walk a schema path (`config.site.sitemap.entries[]`) into a rendered value. */
function resolveSchemaPath(root, schemaPath) {
  let node = root
  for (const raw of schemaPath.split('.').slice(1)) {
    if (node === undefined || node === null) return undefined
    const indexed = raw.endsWith('[]')
    node = node[indexed ? raw.slice(0, -2) : raw]
    if (indexed) node = Array.isArray(node) ? node[0] : undefined
  }
  return node
}

/**
 * A key is accepted by the renderer or it does not exist.
 *
 * `cache.handler`, `cache.maxEntries`, and `cache.maxBytes` were declared by
 * `CONFIG_KEY_SCHEMA`, declared by `RuvyxaConfig`, deserialized by the Rust
 * `ProjectConfig`, and written up in the configuration guide — and `cacheValue()`
 * copied three of the six keys it was given and dropped the rest on the floor.
 * So a project could configure a shared cache handler, pass every check, and
 * have the value reach no consumer on any host: nothing loaded the store, and
 * both memory bounds were lost on the way to the tier they bound.
 *
 * `tests/packages/core/config-schema.test.ts` compares the declarations with
 * each other and cannot see this. Only rendering a config catches a key that
 * every declaration agrees on and nothing emits, which is why this sweep is by
 * section rather than by the one section that was broken.
 */
describe('the config renderer emits every key it accepts', () => {
  it('sets every declared key in the maximal config', () => {
    const missing = []
    for (const [schemaPath, declared] of Object.entries(CONFIG_KEY_SCHEMA)) {
      const section = resolveSchemaPath(MAXIMAL, schemaPath)
      if (section === undefined) {
        missing.push(`${schemaPath} (whole section)`)
        continue
      }
      for (const key of declared) {
        if (schemaPath === 'config' && key in SOURCE_ONLY) continue
        if (section[key] === undefined) missing.push(`${schemaPath}.${key}`)
      }
    }
    assert.deepEqual(
      missing,
      [],
      'MAXIMAL must set every key the schema declares, or the sweep below proves nothing about them',
    )
  })

  it('carries every declared key through to the CLI', async () => {
    const spliced = Object.entries(SOURCE_ONLY)
      .map(([key, source]) => `  ${key}: ${source},`)
      .join('\n')
    const rendered = await render(
      ['export default {', spliced, `  ...${JSON.stringify(MAXIMAL, null, 2)},`, '}', ''].join(
        '\n',
      ),
    )
    const dropped = []
    for (const [schemaPath, declared] of Object.entries(CONFIG_KEY_SCHEMA)) {
      const exempt = new Set(CARRIED_ELSEWHERE[schemaPath] ?? [])
      const section = resolveSchemaPath(rendered, schemaPath)
      for (const key of declared) {
        if (exempt.has(key)) continue
        if (section === undefined || section[key] === undefined) {
          dropped.push(`${schemaPath}.${key}`)
        }
      }
    }
    assert.deepEqual(dropped, [], 'these keys were accepted by the renderer and never emitted')
  })
})

/**
 * A key is accepted by the renderer or it does not exist.
 *
 * `CONFIG_KEY_SCHEMA` accepted `cache.handler`, `cache.maxEntries`, and
 * `cache.maxBytes`; `RuvyxaConfig` declared all three; the Rust `ProjectConfig`
 * deserialized all three; the documentation described all three. And
 * `cacheValue()` — the function that decides what actually leaves this process
 * — copied `routes`, `css`, and `dir` and dropped the rest on the floor. So a
 * project could set a shared cache handler, pass every check, and have the
 * value reach no consumer on any host: the store was never loaded and both
 * memory bounds were lost on the way to the tier they bound.
 *
 * The schema test next door compares the declarations with each other. Only
 * rendering catches a key that every declaration agrees on and nothing emits.
 */
describe('the config renderer emits the cache keys it accepts', () => {
  it('carries handler, maxEntries, and maxBytes through to the CLI', async () => {
    const config = await render(
      `export default {
  appDir: 'app',
  cache: { handler: './cache-handler.mjs', maxEntries: 512, maxBytes: 1048576 },
}
`,
    )
    assert.deepEqual(config.cache, {
      handler: './cache-handler.mjs',
      maxEntries: 512,
      maxBytes: 1048576,
    })
  })

  // Zero is the value that carries the decision in both bounds — no local tier,
  // and no memory ceiling — so a carrier that treated it as "unset" would
  // answer the two questions the feature exists to answer with the default
  // while looking wired.
  it('keeps a zero bound, which is a decision rather than an absence', async () => {
    const config = await render(
      `export default { appDir: 'app', cache: { maxEntries: 0, maxBytes: 0 } }
`,
    )
    assert.equal(config.cache.maxEntries, 0)
    assert.equal(config.cache.maxBytes, 0)
  })

  // The keys this suite asserts are the ones the schema declares, read from the
  // schema rather than written out again: a key added there and forgotten here
  // fails this rather than shipping unrendered.
  it('emits every key the schema declares for the cache section', async () => {
    const declared = [...CONFIG_KEY_SCHEMA['config.cache']].sort()
    const config = await render(
      `export default {
  appDir: 'app',
  cache: {
    routes: false,
    css: false,
    dir: '.cache',
    handler: './cache-handler.mjs',
    maxEntries: 8,
    maxBytes: 16,
  },
}
`,
    )
    assert.deepEqual(Object.keys(config.cache).sort(), declared)
  })

  // A project that configures nothing must still render nothing, so the CLI
  // keeps every default rather than being handed an empty object to interpret.
  it('omits the section when the project configures none of it', async () => {
    const config = await render(`export default { appDir: 'app' }\n`)
    assert.equal(config.cache, undefined)
  })
})
