import { existsSync } from 'node:fs'
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { compareEntryKeys } from './order.mjs'
import { CONFIG_KEY_SCHEMA } from './config-schema.mjs'

import {
  cacheFileName,
  compileBundleIfChanged,
  runtimeAliases,
  serverPlatform,
  toImportPath,
} from './compiler.mjs'

const [projectRootArg] = process.argv.slice(2)

if (!projectRootArg) {
  // Awaited: `fail` is async and only exits after its NDJSON reaches stdout.
  // Calling it bare let execution fall through to `path.resolve(undefined)`
  // one line down, which throws outside the try block below — so the host got
  // an unhandled `ERR_INVALID_ARG_TYPE` stack instead of the RUV1601 line this
  // branch exists to emit.
  await fail('RUV1601', 'Config renderer requires a project root argument.')
}

const projectRoot = path.resolve(projectRootArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))
const SITEMAP_CHANGE_FREQUENCIES = new Set([
  'always',
  'hourly',
  'daily',
  'weekly',
  'monthly',
  'yearly',
  'never',
])

try {
  const configFile = findConfig(projectRoot)
  if (!configFile) {
    await removeRuntimeConfigPointer(projectRoot)
    await ok({}, 'no-config', { inputs: [], env: {} })
  }

  const moduleCode = `export { default } from ${JSON.stringify(toImportPath(configFile))}`
  const outfile = path.join(
    projectRoot,
    '.ruvyxa',
    'cache',
    'config',
    cacheFileName([moduleCode, configFile], 'mjs'),
  )

  const bundle = await compileBundleIfChanged({
    projectRoot,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:config-entry.ts',
    outfile,
    platform: serverPlatform(),
    bundleAliasDependencies: true,
    aliases: runtimeAliases(runtimeDir),
    markdownConfig: false,
  })

  const readEnv = recordEnvironmentReads()
  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
  const config = mod.default ?? {}
  const sanitized = await sanitizeConfig(config)
  await writeRuntimeConfigPointer(projectRoot, bundle.outfile, bundle.dependencyHash)
  await ok(sanitized, bundle.dependencyHash, {
    inputs: bundle.fingerprintInputs,
    env: readEnv(),
  })
} catch (error) {
  await fail('RUV1600', error instanceof Error ? error.message : String(error), error?.stack)
}

function findConfig(root) {
  for (const fileName of [
    'ruvyxa.config.ts',
    'ruvyxa.config.mts',
    'ruvyxa.config.js',
    'ruvyxa.config.mjs',
  ]) {
    const file = path.join(root, fileName)
    if (existsSync(file)) return file
  }
  return null
}

/**
 * Reject a key `ruvyxa.config` does not define, at every level.
 *
 * The names are data in `config-schema.mjs` rather than argument lists here.
 * `RuvyxaConfig` in `@ruvyxa/core` describes the same surface for the type
 * checker, nothing held the two together, and they drifted: `build.target` was
 * accepted here, validated by the Rust config, and applied by both compilers
 * while the public type never declared it, so a project that set it failed
 * `tsc` against a build that honoured it. One table, two readers.
 *
 * Split by section because the projection below is the part worth reading;
 * these four functions are now only the shape of the walk.
 */
function assertConfigKeys(config) {
  assertTopLevelConfigKeys(config)
  assertRuntimeConfigKeys(config)
  assertSiteConfigKeys(config)
  assertMiddlewareConfigKeys(config)
}

/** The option names `ruvyxa.config` accepts at its root. */
function assertTopLevelConfigKeys(config) {
  assertKnownKeys(config, 'config')
}

/** Compiler, server, build, image, i18n, security, and cache sections. */
function assertRuntimeConfigKeys(config) {
  assertKnownKeys(config.css, 'config.css')
  assertKnownKeys(config.markdown, 'config.markdown')
  assertKnownKeys(config.server, 'config.server')
  assertKnownKeys(config.build, 'config.build')
  assertKnownKeys(config.debug, 'config.debug')
  assertKnownKeys(config.image, 'config.image')
  assertKnownKeys(config.image?.onDemand, 'config.image.onDemand')
  assertKnownKeys(config.i18n, 'config.i18n')
  assertKnownKeys(config.security, 'config.security')
  assertKnownKeys(config.security?.actionRateLimit, 'config.security.actionRateLimit')
  assertKnownKeys(config.cache, 'config.cache')
}

/** Site metadata: the content engine, sitemap entries, and robots rules. */
function assertSiteConfigKeys(config) {
  assertKnownKeys(config.site, 'config.site')
  assertKnownKeys(config.content, 'config.content')
  if (isObject(config.content?.engine)) {
    assertKnownKeys(config.content.engine, 'config.content.engine')
  }
  assertKnownKeys(config.site?.sitemap, 'config.site.sitemap')
  assertKnownKeys(config.site?.sitemap?.defaults, 'config.site.sitemap.defaults')
  // One schema entry describes every element of an array; the field name the
  // user is shown carries the index instead, so the diagnostic still points at
  // the entry they wrote.
  const entrySchema = 'config.site.sitemap.entries[]'
  for (const [index, entry] of sitemapEntries(config.site?.sitemap?.entries).entries()) {
    const field = `config.site.sitemap.entries[${index}]`
    assertKnownKeys(entry, entrySchema, field)
    assertKnownKeys(entry?.alternates, `${entrySchema}.alternates`, `${field}.alternates`)
    const videoSchema = `${entrySchema}.videos[]`
    for (const [videoIndex, video] of sitemapVideos(entry?.videos).entries()) {
      const videoField = `${field}.videos[${videoIndex}]`
      assertKnownKeys(video, videoSchema, videoField)
      assertKnownKeys(video?.restriction, `${videoSchema}.restriction`, `${videoField}.restriction`)
      assertKnownKeys(video?.platform, `${videoSchema}.platform`, `${videoField}.platform`)
      assertKnownKeys(video?.uploader, `${videoSchema}.uploader`, `${videoField}.uploader`)
    }
  }
  assertKnownKeys(config.site?.robots, 'config.site.robots')
  for (const [index, rule] of siteRobotsRules(config.site?.robots?.rules).entries()) {
    assertKnownKeys(rule, 'config.site.robots.rules[]', `config.site.robots.rules[${index}]`)
  }
}

/** Render defaults and the built-in middleware stack. */
function assertMiddlewareConfigKeys(config) {
  assertKnownKeys(config.render, 'config.render')
  assertKnownKeys(config.middleware, 'config.middleware')
  assertKnownKeys(config.middleware?.builtin, 'config.middleware.builtin')
  assertKnownKeys(config.middleware?.builtin?.cors, 'config.middleware.builtin.cors')
  assertKnownKeys(config.middleware?.builtin?.rate, 'config.middleware.builtin.rate')
}

async function sanitizeConfig(config) {
  assertConfigKeys(config)
  assertConfigValueShape(config)
  assertMarkdownShape(config.markdown)
  assertContentShape(config.content, config.site)

  return {
    appDir: stringValue(config.appDir),
    outDir: stringValue(config.outDir),
    runtime: stringValue(config.runtime),
    react: booleanValue(config.react),
    reactCompiler: booleanValue(config.reactCompiler),
    typedRoutes: booleanValue(config.typedRoutes),
    css: objectValue(config.css, { entries: stringArrayValue(config.css?.entries) }),
    // Executable unified plugins remain in the compiled config module. Rust
    // only needs to know whether to activate the persistent content bridge.
    markdown: config.markdown === undefined ? undefined : true,
    server: serverValue(config.server),
    build: buildValue(config.build),
    render: renderValue(config.render),
    debug: debugValue(config.debug),
    image: imageValue(config.image),
    i18n: i18nValue(config.i18n),
    security: securityValue(config.security),
    cache: cacheValue(config.cache),
    site: siteValue(config.site),
    content: contentValue(config.content),
    middleware: safeJsonValue(config.middleware),
    adapter: await adapterOutput(config.adapter, projectRoot, config.outDir),
    adapterOptions: safeJsonValue(config.adapterOptions),
    plugins: pluginDescriptors(config.plugins, config.content),
  }
}

/*
 * One projection helper per config section.
 *
 * Each `?.` is a branch, so the flat object literal these came from carried the
 * complexity of the whole config surface in a single function while expressing
 * one idea: read a value, coerce it, drop it if absent. Per-section helpers put
 * each group next to the assertions that validated it, and match the
 * `siteValue` / `contentValue` / `imageOnDemandValue` helpers already here.
 */

function serverValue(server) {
  return objectValue(server, {
    host: stringValue(server?.host),
    port: numberValue(server?.port),
  })
}

function buildValue(build) {
  return objectValue(build, {
    minify: booleanValue(build?.minify),
    map: booleanValue(build?.map),
    treeShake: booleanValue(build?.treeShake),
    split: stringValue(build?.split),
    workers: numberValue(build?.workers),
    jsx: stringValue(build?.jsx),
    target: stringValue(build?.target),
    manifest: booleanValue(build?.manifest),
    warm: booleanValue(build?.warm),
    prerenderCache: booleanValue(build?.prerenderCache),
  })
}

function renderValue(render) {
  return objectValue(render, {
    strategy: stringValue(render?.strategy),
    revalidate: numberValue(render?.revalidate),
  })
}

function debugValue(debug) {
  return objectValue(debug, {
    overlay: booleanValue(debug?.overlay),
    traces: booleanValue(debug?.traces),
  })
}

function imageValue(image) {
  return objectValue(image, {
    optimize: booleanValue(image?.optimize),
    quality: numberValue(image?.quality),
    lossless: booleanValue(image?.lossless),
    keepOriginal: booleanValue(image?.keepOriginal),
    variantWidths: numberArrayValue(image?.variantWidths),
    // `0` is the documented "publish the source's own resolution" value, so
    // this has to survive the projection as the number it is: `numberValue`
    // keeps a zero and drops only `undefined`.
    maxWidth: numberValue(image?.maxWidth),
    workers: numberValue(image?.workers),
    effort: numberValue(image?.effort),
    onDemand: imageOnDemandValue(image?.onDemand),
  })
}

function i18nValue(i18n) {
  return objectValue(i18n, {
    locales: stringArrayValue(i18n?.locales),
    defaultLocale: stringValue(i18n?.defaultLocale),
    localeParam: stringValue(i18n?.localeParam),
    detectLocale: booleanValue(i18n?.detectLocale),
    cookie: stringValue(i18n?.cookie),
  })
}

function securityValue(security) {
  return objectValue(security, {
    actionLimit: numberValue(security?.actionLimit),
    apiLimit: numberValue(security?.apiLimit),
    pluginLimit: numberValue(security?.pluginLimit),
    actionRateLimit: objectValue(security?.actionRateLimit, {
      max: numberValue(security?.actionRateLimit?.max),
      window: numberValue(security?.actionRateLimit?.window),
    }),
    sameOrigin: booleanValue(security?.sameOrigin),
    fetchMeta: booleanValue(security?.fetchMeta),
    trustedProxyIps: stringArrayValue(security?.trustedProxyIps),
    headers: booleanValue(security?.headers),
  })
}

function cacheValue(cache) {
  return objectValue(cache, {
    routes: booleanValue(cache?.routes),
    css: booleanValue(cache?.css),
    dir: stringValue(cache?.dir),
    // The three the shared cache is made of. `CONFIG_KEY_SCHEMA` accepted them
    // and `RuvyxaConfig` declared them, so a project setting one passed every
    // check — and this function decides what actually leaves the renderer, so
    // none of the three ever reached a consumer. `cache.handler` named a store
    // nothing loaded, and both bounds were dropped on the way to the tier they
    // bound. A key is accepted here or it does not exist.
    handler: stringValue(cache?.handler),
    // `numberValue` keeps `0`, which both bounds use as a decision: zero
    // entries turns the local tier off, zero bytes removes the memory ceiling.
    maxEntries: numberValue(cache?.maxEntries),
    maxBytes: numberValue(cache?.maxBytes),
  })
}

function assertMarkdownShape(markdown) {
  if (markdown === undefined) return
  if (!isObject(markdown)) throw new Error('RUV1602 config.markdown must be an object.')
  if (markdown.gfm !== undefined && typeof markdown.gfm !== 'boolean') {
    throw new Error('RUV1602 config.markdown.gfm must be boolean.')
  }
  for (const field of ['remarkPlugins', 'rehypePlugins', 'recmaPlugins']) {
    if (markdown[field] !== undefined && !Array.isArray(markdown[field])) {
      throw new Error(`RUV1602 config.markdown.${field} must be an array.`)
    }
  }
  if (markdown.remarkRehypeOptions !== undefined && !isObject(markdown.remarkRehypeOptions)) {
    throw new Error('RUV1602 config.markdown.remarkRehypeOptions must be an object.')
  }
}

async function writeRuntimeConfigPointer(root, bundleFile, dependencyHash) {
  const directory = path.join(root, '.ruvyxa', 'cache', 'config')
  const pointer = path.join(directory, 'runtime-config.mjs')
  let specifier = path.relative(directory, bundleFile).replaceAll('\\', '/')
  if (!specifier.startsWith('.')) specifier = `./${specifier}`
  const versioned = JSON.stringify(`${specifier}?v=${dependencyHash}`)
  // `default` stays the Markdown configuration, which is what the compiler has
  // always imported from here. `plugins` is added beside it so the JavaScript
  // compiler can run the project's `build.onTransform` hooks: those reached the
  // Rust bundler alone, so a plugin rewrote the browser bundle while every
  // server render read the original file, and a rewritten value that landed in
  // markup made the two documents disagree.
  const source =
    `import config from ${versioned}\n` +
    `export default config?.markdown\n` +
    `export const plugins = config?.plugins ?? []\n` +
    `export const dependencyHash = ${JSON.stringify(dependencyHash)}\n`
  await mkdir(directory, { recursive: true })
  try {
    if ((await readFile(pointer, 'utf8')) === source) return
  } catch {
    // The pointer is generated on the first successful config render.
  }
  await writeFile(pointer, source)
}

async function removeRuntimeConfigPointer(root) {
  await rm(path.join(root, '.ruvyxa', 'cache', 'config', 'runtime-config.mjs'), { force: true })
}

function assertConfigValueShape(config) {
  assertShape(config, 'config', {
    appDir: 'string',
    outDir: 'string',
    runtime: 'string',
    react: 'boolean',
    reactCompiler: 'boolean',
    typedRoutes: 'boolean',
    typescript: { strict: 'boolean' },
    css: { entries: 'string[]' },
    server: { host: 'string', port: 'number' },
    build: {
      minify: 'boolean',
      map: 'boolean',
      treeShake: 'boolean',
      split: 'string',
      workers: 'number',
      jsx: 'string',
      target: 'string',
      manifest: 'boolean',
      warm: 'boolean',
      prerenderCache: 'boolean',
    },
    render: { strategy: 'string', revalidate: 'number' },
    debug: { overlay: 'boolean', traces: 'boolean' },
    image: {
      optimize: 'boolean',
      quality: 'number',
      lossless: 'boolean',
      keepOriginal: 'boolean',
      variantWidths: 'number[]',
      maxWidth: 'number',
      workers: 'number',
      effort: 'number',
    },
    i18n: {
      locales: 'string[]',
      defaultLocale: 'string',
      localeParam: 'string',
      detectLocale: 'boolean',
      cookie: 'string',
    },
    security: {
      actionLimit: 'number',
      apiLimit: 'number',
      pluginLimit: 'number',
      actionRateLimit: { max: 'number', window: 'number' },
      sameOrigin: 'boolean',
      fetchMeta: 'boolean',
      trustedProxyIps: 'string[]',
      headers: 'boolean',
    },
    cache: {
      routes: 'boolean',
      css: 'boolean',
      dir: 'string',
      handler: 'string',
      maxEntries: 'number',
      maxBytes: 'number',
    },
    middleware: { workers: 'number', timeoutMs: 'number' },
    adapter: 'object',
    adapterOptions: 'object',
    plugins: 'array',
  })
  assertSiteShape(config.site)
  assertImageOnDemandShape(config.image?.onDemand)
}

function assertImageOnDemandShape(value) {
  if (value === undefined || typeof value === 'boolean') return
  assertShape(value, 'config.image.onDemand', { enabled: 'boolean', maxWidth: 'number' })
}

/** `config.site.sitemap`: `true` for the defaults, or the long form. */
function assertSitemapShape(sitemap) {
  if (sitemap === undefined) return
  if (typeof sitemap !== 'boolean' && !isObject(sitemap)) {
    throw new Error('RUV1602 config.site.sitemap must be boolean or object.')
  }
  if (!isObject(sitemap)) return

  assertStringArray(sitemap.exclude, 'config.site.sitemap.exclude')
  assertStringArray(sitemap.additionalPaths, 'config.site.sitemap.additionalPaths')
  if (sitemap.defaults !== undefined && !isObject(sitemap.defaults)) {
    throw new Error('RUV1602 config.site.sitemap.defaults must be an object.')
  }
  if (isObject(sitemap.defaults)) {
    assertSitemapEntryMetadata(sitemap.defaults, 'config.site.sitemap.defaults')
  }
  if (sitemap.entries !== undefined && !Array.isArray(sitemap.entries)) {
    throw new Error('RUV1602 config.site.sitemap.entries must be an array.')
  }
  for (const [index, entry] of sitemapEntries(sitemap.entries).entries()) {
    assertSitemapEntry(entry, `config.site.sitemap.entries[${index}]`)
  }
}

/** One `config.site.robots.rules` entry. */
function assertRobotsRule(rule, field) {
  if (!isObject(rule)) throw new Error(`RUV1602 ${field} must be an object.`)
  assertStringOrArray(rule.userAgent, `${field}.userAgent`)
  assertStringOrArray(rule.allow, `${field}.allow`)
  assertStringOrArray(rule.disallow, `${field}.disallow`)
  if (
    rule.crawlDelay !== undefined &&
    (!Number.isSafeInteger(rule.crawlDelay) || rule.crawlDelay < 0)
  ) {
    throw new Error(`RUV1602 ${field}.crawlDelay must be a non-negative safe integer.`)
  }
}

/** `config.site.robots`: `true` for the defaults, or the long form. */
function assertRobotsShape(robots) {
  if (robots === undefined) return
  if (typeof robots !== 'boolean' && !isObject(robots)) {
    throw new Error('RUV1602 config.site.robots must be boolean or object.')
  }
  if (!isObject(robots)) return

  assertStringOrArray(robots.sitemap, 'config.site.robots.sitemap')
  if (robots.host !== undefined && typeof robots.host !== 'string') {
    throw new Error('RUV1602 config.site.robots.host must be string.')
  }
  const rules = siteRobotsRules(robots.rules)
  if (robots.rules !== undefined && !isObject(robots.rules) && !Array.isArray(robots.rules)) {
    throw new Error('RUV1602 config.site.robots.rules must be object or array.')
  }
  for (const [index, rule] of rules.entries()) {
    assertRobotsRule(rule, `config.site.robots.rules[${index}]`)
  }
}

function assertSiteShape(site) {
  if (site === undefined) return
  if (!isObject(site)) throw new Error('RUV1602 config.site must be an object.')
  if (site.url !== undefined && typeof site.url !== 'string') {
    throw new Error('RUV1602 config.site.url must be string.')
  }
  for (const field of ['title', 'description', 'language']) {
    if (site[field] !== undefined && typeof site[field] !== 'string') {
      throw new Error(`RUV1602 config.site.${field} must be string.`)
    }
  }
  assertSitemapShape(site.sitemap)
  assertRobotsShape(site.robots)
}

function assertContentShape(content, site) {
  if (content === undefined || content === false) return
  if (content !== true && !isObject(content)) {
    throw new Error('RUV1602 config.content must be boolean or object.')
  }
  const engine = content === true ? true : content.engine
  if (engine !== undefined && typeof engine !== 'boolean' && !isObject(engine)) {
    throw new Error('RUV1602 config.content.engine must be boolean or object.')
  }
  if (!contentEngineEnabled(content)) return

  for (const field of ['url', 'title', 'description']) {
    if (typeof site?.[field] !== 'string' || site[field].trim() === '') {
      throw new Error(
        `RUV1602 config.site.${field} must be a non-empty string when content engine is enabled.`,
      )
    }
  }
  if (site.language !== undefined) assertLocale(site.language, 'config.site.language')
  if (!isObject(engine)) return
  assertStringArray(engine.exclude, 'config.content.engine.exclude')
  assertStringArray(engine.stopWords, 'config.content.engine.stopWords')
  for (const field of [
    'locale',
    'manifestPath',
    'searchPath',
    'feedPath',
    'sitemapPath',
    'language',
  ]) {
    if (engine[field] !== undefined && typeof engine[field] !== 'string') {
      throw new Error(`RUV1602 config.content.engine.${field} must be string.`)
    }
  }
  if (engine.locale !== undefined) assertLocale(engine.locale, 'config.content.engine.locale')
  if (engine.language !== undefined) assertLocale(engine.language, 'config.content.engine.language')
  if (
    engine.minTermLength !== undefined &&
    (!Number.isSafeInteger(engine.minTermLength) ||
      engine.minTermLength < 1 ||
      engine.minTermLength > 64)
  ) {
    throw new Error('RUV1602 config.content.engine.minTermLength must be an integer from 1 to 64.')
  }
  if (
    engine.llmsPath !== undefined &&
    engine.llmsPath !== false &&
    typeof engine.llmsPath !== 'string'
  ) {
    throw new Error('RUV1602 config.content.engine.llmsPath must be string or false.')
  }
}

function assertLocale(value, field) {
  try {
    Intl.Segmenter.supportedLocalesOf(value)
  } catch {
    throw new Error(`RUV1602 ${field} must be a valid BCP 47 locale.`)
  }
}

function assertStringArray(value, field) {
  if (value === undefined) return
  if (!Array.isArray(value) || !value.every((item) => typeof item === 'string')) {
    throw new Error(`RUV1602 ${field} must be string[].`)
  }
}

function assertStringOrArray(value, field) {
  if (value === undefined) return
  if (
    typeof value !== 'string' &&
    (!Array.isArray(value) || !value.every((item) => typeof item === 'string'))
  ) {
    throw new Error(`RUV1602 ${field} must be string or string[].`)
  }
}

function siteRobotsRules(value) {
  if (value === undefined) return []
  return Array.isArray(value) ? value : [value]
}

function sitemapEntries(value) {
  return Array.isArray(value) ? value : []
}

function sitemapVideos(value) {
  return Array.isArray(value) ? value : []
}

function assertSitemapEntry(entry, field) {
  if (!isObject(entry)) throw new Error(`RUV1602 ${field} must be an object.`)
  if (typeof entry.url !== 'string' || entry.url === '') {
    throw new Error(`RUV1602 ${field}.url must be a non-empty string.`)
  }
  assertSitemapEntryMetadata(entry, field)
  if (entry.alternates !== undefined && !isObject(entry.alternates)) {
    throw new Error(`RUV1602 ${field}.alternates must be an object.`)
  }
  if (entry.alternates?.languages !== undefined) {
    if (
      !isObject(entry.alternates.languages) ||
      !Object.values(entry.alternates.languages).every((value) => typeof value === 'string')
    ) {
      throw new Error(`RUV1602 ${field}.alternates.languages must be a string record.`)
    }
  }
  assertStringArray(entry.images, `${field}.images`)
  if (entry.videos !== undefined && !Array.isArray(entry.videos)) {
    throw new Error(`RUV1602 ${field}.videos must be an array.`)
  }
  for (const [index, video] of sitemapVideos(entry.videos).entries()) {
    assertSitemapVideo(video, `${field}.videos[${index}]`)
  }
}

function assertSitemapEntryMetadata(value, field) {
  assertDateValue(value.lastModified, `${field}.lastModified`)
  if (
    value.changeFrequency !== undefined &&
    !SITEMAP_CHANGE_FREQUENCIES.has(value.changeFrequency)
  ) {
    throw new Error(`RUV1602 ${field}.changeFrequency must be a supported sitemap frequency.`)
  }
  if (
    value.priority !== undefined &&
    (!Number.isFinite(value.priority) || value.priority < 0 || value.priority > 1)
  ) {
    throw new Error(`RUV1602 ${field}.priority must be between 0 and 1.`)
  }
}

function assertSitemapVideo(video, field) {
  if (!isObject(video)) throw new Error(`RUV1602 ${field} must be an object.`)
  for (const key of ['title', 'thumbnail_loc', 'description']) {
    if (typeof video[key] !== 'string' || video[key] === '') {
      throw new Error(`RUV1602 ${field}.${key} must be a non-empty string.`)
    }
  }
  for (const key of ['content_loc', 'player_loc']) {
    if (video[key] !== undefined && typeof video[key] !== 'string') {
      throw new Error(`RUV1602 ${field}.${key} must be a string.`)
    }
  }
  for (const key of ['duration', 'view_count', 'rating']) {
    if (video[key] !== undefined && !Number.isFinite(video[key])) {
      throw new Error(`RUV1602 ${field}.${key} must be a finite number.`)
    }
  }
  assertDateValue(video.expiration_date, `${field}.expiration_date`)
  assertDateValue(video.publication_date, `${field}.publication_date`)
  for (const key of ['family_friendly', 'requires_subscription', 'live']) {
    if (video[key] !== undefined && video[key] !== 'yes' && video[key] !== 'no') {
      throw new Error(`RUV1602 ${field}.${key} must be "yes" or "no".`)
    }
  }
  for (const key of ['restriction', 'platform']) {
    const relationship = video[key]
    if (relationship === undefined) continue
    if (
      !isObject(relationship) ||
      (relationship.relationship !== 'allow' && relationship.relationship !== 'deny') ||
      typeof relationship.content !== 'string' ||
      relationship.content === ''
    ) {
      throw new Error(
        `RUV1602 ${field}.${key} must contain relationship "allow" or "deny" and string content.`,
      )
    }
  }
  if (video.uploader !== undefined) {
    if (
      !isObject(video.uploader) ||
      typeof video.uploader.content !== 'string' ||
      video.uploader.content === '' ||
      (video.uploader.info !== undefined && typeof video.uploader.info !== 'string')
    ) {
      throw new Error(`RUV1602 ${field}.uploader must contain string content and optional info.`)
    }
  }
  assertStringOrArray(video.tag, `${field}.tag`)
}

function assertDateValue(value, field) {
  if (value === undefined) return
  if (value instanceof Date) {
    if (!Number.isFinite(value.getTime())) throw new Error(`RUV1602 ${field} must be a valid Date.`)
    return
  }
  if (typeof value !== 'string') {
    throw new Error(`RUV1602 ${field} must be a string or Date.`)
  }
}

function siteValue(site) {
  if (!isObject(site)) return undefined
  return objectValue(site, {
    url: stringValue(site.url),
    title: stringValue(site.title),
    description: stringValue(site.description),
    language: stringValue(site.language),
    sitemap: siteSettingValue(site.sitemap),
    robots: siteSettingValue(site.robots),
  })
}

function contentValue(content) {
  if (typeof content === 'boolean') return content
  return isObject(content) ? safeJsonValue(content) : undefined
}

function siteSettingValue(value) {
  if (typeof value === 'boolean') return value
  return isObject(value) ? safeJsonValue(value) : undefined
}

function imageOnDemandValue(value) {
  if (typeof value === 'boolean') return value
  return isObject(value)
    ? objectValue(value, {
        enabled: booleanValue(value.enabled),
        maxWidth: numberValue(value.maxWidth),
      })
    : undefined
}

function assertShape(value, field, shape) {
  if (!isObject(value)) {
    throw new Error(`RUV1602 ${field} must be an object.`)
  }
  for (const [key, expected] of Object.entries(shape)) {
    const candidate = value[key]
    if (candidate === undefined) continue
    const childField = `${field}.${key}`
    if (typeof expected === 'object') {
      assertShape(candidate, childField, expected)
      continue
    }
    const valid =
      (expected === 'string' && typeof candidate === 'string') ||
      (expected === 'number' && Number.isFinite(candidate)) ||
      (expected === 'boolean' && typeof candidate === 'boolean') ||
      (expected === 'object' && isObject(candidate)) ||
      (expected === 'array' && Array.isArray(candidate)) ||
      (expected === 'string[]' &&
        Array.isArray(candidate) &&
        candidate.every((item) => typeof item === 'string')) ||
      (expected === 'number[]' &&
        Array.isArray(candidate) &&
        candidate.every((item) => Number.isFinite(item)))
    if (!valid) {
      throw new Error(`RUV1602 ${childField} must be ${expected}.`)
    }
  }
}

function isObject(value) {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}

async function adapterOutput(adapter, root, outDir) {
  if (adapter === undefined) return undefined
  if (!adapter || typeof adapter !== 'object' || typeof adapter.build !== 'function') {
    throw new Error('RUV1603 config.adapter must provide a build(context) function.')
  }

  const output = await adapter.build({ root, outDir: stringValue(outDir) ?? '.ruvyxa' })
  if (!output || typeof output !== 'object') {
    throw new Error('RUV1603 config.adapter.build(context) must return an adapter output object.')
  }
  if (typeof output.name !== 'string' || typeof output.target !== 'string') {
    throw new Error('RUV1603 adapter output must include string name and target fields.')
  }

  const serialized = safeJsonValue(output)
  if (serialized === undefined) {
    throw new Error('RUV1603 adapter output must be JSON-serializable.')
  }

  return serialized
}

/**
 * Reject any key of `value` that `schemaPath` does not name.
 *
 * `field` is what the user is shown, and defaults to the schema path. They
 * differ for one element of an array, where the schema describes the shape once
 * and the diagnostic has to name the index the user wrote.
 */
function assertKnownKeys(value, schemaPath, field = schemaPath) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return
  const allowedKeys = CONFIG_KEY_SCHEMA[schemaPath]
  if (allowedKeys === undefined) {
    // A section checked against a path the schema does not define would accept
    // every key silently, which is the whole failure this function prevents.
    throw new Error(`RUV1602 config schema has no entry for ${schemaPath}`)
  }
  const allowed = new Set(allowedKeys)
  const unknown = Object.keys(value).filter((key) => !allowed.has(key))
  if (unknown.length > 0) {
    const listed = unknown.join(', ')
    // The schema already holds every legal key for this section, so a typo can
    // be answered rather than merely refused. `unknown config field: appDirr`
    // told the reader what they had already typed and left them to diff it
    // against the documentation by eye.
    const suggestion = nearestConfigKey(unknown[0], allowedKeys)
    const hint = suggestion
      ? ` Did you mean \`${suggestion}\`?`
      : ` Known ${field} fields: ${allowedKeys.join(', ')}.`
    throw new Error(
      `RUV1602 unknown ${field} field${unknown.length === 1 ? '' : 's'}: ${listed}.${hint}`,
    )
  }
}

/**
 * The legal key closest to `typed`, or `undefined` when nothing is close.
 *
 * The threshold scales with length so a short key cannot match an unrelated
 * short key: `ppr` and `jsx` are three edits apart on a three-letter word and
 * suggesting one for the other would be worse than saying nothing. Above that
 * the caller lists the whole section, which is short by construction.
 */
function nearestConfigKey(typed, allowedKeys) {
  const budget = Math.max(1, Math.floor(typed.length / 3))
  let best
  let bestDistance = Infinity
  for (const candidate of allowedKeys) {
    const distance = editDistance(typed.toLowerCase(), candidate.toLowerCase())
    if (distance < bestDistance) {
      bestDistance = distance
      best = candidate
    }
  }
  return bestDistance <= budget ? best : undefined
}

/** Levenshtein distance, two rows at a time. */
function editDistance(left, right) {
  let previous = Array.from({ length: right.length + 1 }, (_, index) => index)
  for (let i = 1; i <= left.length; i += 1) {
    const current = [i]
    for (let j = 1; j <= right.length; j += 1) {
      const substitution = previous[j - 1] + (left[i - 1] === right[j - 1] ? 0 : 1)
      current[j] = Math.min(substitution, previous[j] + 1, current[j - 1] + 1)
    }
    previous = current
  }
  return previous[right.length]
}

function objectValue(source, value) {
  if (!source || typeof source !== 'object') return undefined
  const filtered = Object.fromEntries(
    Object.entries(value).filter(([, item]) => item !== undefined),
  )
  return Object.keys(filtered).length > 0 ? filtered : undefined
}

function stringValue(value) {
  return typeof value === 'string' ? value : undefined
}

function numberValue(value) {
  return Number.isFinite(value) ? value : undefined
}

function numberArrayValue(value) {
  if (!Array.isArray(value) || !value.every((item) => Number.isFinite(item))) return undefined
  return value
}

function booleanValue(value) {
  return typeof value === 'boolean' ? value : undefined
}

function stringArrayValue(value) {
  if (!Array.isArray(value) || !value.every((item) => typeof item === 'string')) return undefined
  return value
}

function safeJsonValue(value) {
  if (value === undefined) return undefined
  try {
    JSON.stringify(value)
    return value
  } catch {
    return undefined
  }
}

function pluginDescriptors(value, content) {
  const names = new Set()
  const plugins = (Array.isArray(value) ? value : []).map((plugin, index) => {
    if (!isObject(plugin)) {
      throw new Error(`RUV1602 config.plugins[${index}] must be an object.`)
    }
    if (typeof plugin.name !== 'string' || plugin.name.trim() === '') {
      throw new Error(`RUV1602 config.plugins[${index}].name must be a non-empty string.`)
    }
    if (typeof plugin.register !== 'function') {
      throw new Error(`RUV1602 plugin "${plugin.name}" must provide register(api).`)
    }
    const name = plugin.name.trim()
    if (names.has(name)) {
      throw new Error(`RUV1602 duplicate plugin name: ${name}`)
    }
    names.add(name)
    // Head entries are declared once and injected by the server on every
    // render, so they travel with the descriptor instead of through a
    // per-request hook. `definePlugin` has already validated their shape.
    const head = Array.isArray(plugin.head) ? plugin.head.filter(isObject) : []
    return head.length > 0 ? { name, head } : { name }
  })

  if (contentEngineEnabled(content)) {
    const name = 'ruvyxa:content-engine'
    if (names.has(name)) {
      throw new Error(
        'RUV1602 content engine is configured twice; use either config.content or contentEngine().',
      )
    }
    plugins.push({ name })
  }

  return plugins.length > 0 ? plugins : undefined
}

function contentEngineEnabled(content) {
  if (content === true) return true
  return isObject(content) && (content.engine === true || isObject(content.engine))
}

/**
 * Replace `process.env` with a recorder for the duration of config evaluation.
 *
 * The CLI caches a rendered config and skips this process entirely while the
 * inputs are unchanged. A config that branches on an environment variable would
 * be frozen at whatever the variable was on the run that populated the cache —
 * so the cache key has to include those variables. Recording the reads keeps
 * that key exact: a config that reads nothing pins nothing, and one that reads
 * `NODE_ENV` re-renders when `NODE_ENV` changes and not otherwise.
 *
 * A missing variable is recorded as `null` and is just as load-bearing as a
 * present one: `process.env.ANALYZE ? ... : ...` must re-render when the
 * variable appears, not only when its value changes.
 *
 * @returns {() => Record<string, string|null>} Reads observed so far.
 */
function recordEnvironmentReads() {
  const source = process.env
  const observed = new Map()
  const observe = (key) => {
    if (typeof key !== 'string' || observed.has(key)) return
    observed.set(key, Object.hasOwn(source, key) ? source[key] : null)
  }

  const proxy = new Proxy(source, {
    get(target, key, receiver) {
      observe(key)
      return Reflect.get(target, key, receiver)
    },
    has(target, key) {
      observe(key)
      return Reflect.has(target, key)
    },
    // Enumeration reads every value at once, so the whole environment becomes
    // part of the key. Rare, and better than a config that silently goes stale.
    ownKeys(target) {
      for (const key of Reflect.ownKeys(target)) observe(key)
      return Reflect.ownKeys(target)
    },
  })

  Object.defineProperty(process, 'env', { value: proxy, configurable: true, writable: true })

  return () => {
    Object.defineProperty(process, 'env', { value: source, configurable: true, writable: true })
    return Object.fromEntries([...observed].sort(compareEntryKeys))
  }
}

async function ok(config, dependencyHash, cacheKey = { inputs: [], env: {} }) {
  await writeJson({ ok: true, config, dependencyHash, cacheKey })
  process.exit(0)
}

async function fail(code, message, stack) {
  await writeJson({ ok: false, code, message, stack })
  process.exit(1)
}

function writeJson(payload) {
  return new Promise((resolve, reject) => {
    process.stdout.write(JSON.stringify(payload), (error) => {
      if (error) {
        reject(error)
      } else {
        resolve()
      }
    })
  })
}
