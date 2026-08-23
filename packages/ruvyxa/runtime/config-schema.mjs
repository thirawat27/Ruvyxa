/**
 * The option names `ruvyxa.config` accepts, at every level.
 *
 * This is the authority for what a user may write, and it exists as data rather
 * than as twenty `assertKnownKeys` call sites so a second reader can check it.
 * `RuvyxaConfig` in `@ruvyxa/core` describes the same object for the type
 * checker, and nothing held the two together: `build.target` was accepted here,
 * validated by the Rust config, applied by both compilers, and documented — and
 * absent from `RuvyxaConfig`, so a project that set it failed `tsc` while the
 * build honoured it. `tests/packages/core/config-schema.test.ts` replays this
 * table against a `RuvyxaConfig`-typed literal in both directions.
 *
 * A key is a section path. `[]` marks the schema of one element of an array —
 * the concrete field name reported to the user carries the index instead.
 *
 * This is not a cache stamp and carries no version: it is a description of the
 * current config surface, and the test that reads it fails the moment the two
 * descriptions disagree.
 */
export const CONFIG_KEY_SCHEMA = Object.freeze({
  config: [
    'appDir',
    'outDir',
    'runtime',
    'react',
    'reactCompiler',
    'typescript',
    'typedRoutes',
    'css',
    'markdown',
    'server',
    'build',
    'render',
    'debug',
    'image',
    'i18n',
    'security',
    'cache',
    'site',
    'content',
    'middleware',
    'adapter',
    'adapterOptions',
    'plugins',
  ],
  'config.css': ['entries'],
  'config.markdown': [
    'gfm',
    'remarkPlugins',
    'rehypePlugins',
    'recmaPlugins',
    'remarkRehypeOptions',
  ],
  'config.server': ['host', 'port'],
  'config.build': [
    'minify',
    'map',
    'treeShake',
    'split',
    'workers',
    'jsx',
    'target',
    'manifest',
    'warm',
    'prerenderCache',
  ],
  'config.render': ['strategy', 'revalidate'],
  'config.debug': ['overlay', 'traces'],
  'config.image': [
    'optimize',
    'quality',
    'lossless',
    'keepOriginal',
    'variantWidths',
    'workers',
    'effort',
    'onDemand',
  ],
  'config.image.onDemand': ['enabled', 'maxWidth'],
  'config.i18n': ['locales', 'defaultLocale', 'localeParam', 'detectLocale', 'cookie'],
  'config.security': [
    'actionLimit',
    'apiLimit',
    'pluginLimit',
    'actionRateLimit',
    'sameOrigin',
    'fetchMeta',
    'trustedProxyIps',
    'headers',
  ],
  'config.security.actionRateLimit': ['max', 'window'],
  'config.cache': ['routes', 'css', 'dir'],
  'config.site': ['url', 'title', 'description', 'language', 'sitemap', 'robots'],
  'config.site.sitemap': ['exclude', 'additionalPaths', 'defaults', 'entries'],
  'config.site.sitemap.defaults': ['lastModified', 'changeFrequency', 'priority'],
  'config.site.sitemap.entries[]': [
    'url',
    'lastModified',
    'changeFrequency',
    'priority',
    'alternates',
    'images',
    'videos',
  ],
  'config.site.sitemap.entries[].alternates': ['languages'],
  'config.site.sitemap.entries[].videos[]': [
    'title',
    'thumbnail_loc',
    'description',
    'content_loc',
    'player_loc',
    'duration',
    'view_count',
    'rating',
    'expiration_date',
    'publication_date',
    'family_friendly',
    'requires_subscription',
    'live',
    'restriction',
    'platform',
    'uploader',
    'tag',
  ],
  'config.site.sitemap.entries[].videos[].restriction': ['relationship', 'content'],
  'config.site.sitemap.entries[].videos[].platform': ['relationship', 'content'],
  'config.site.sitemap.entries[].videos[].uploader': ['content', 'info'],
  'config.site.robots': ['rules', 'sitemap', 'host'],
  'config.site.robots.rules[]': ['userAgent', 'allow', 'disallow', 'crawlDelay'],
  'config.content': ['engine'],
  'config.content.engine': [
    'exclude',
    'locale',
    'stopWords',
    'minTermLength',
    'manifestPath',
    'searchPath',
    'feedPath',
    'sitemapPath',
    'llmsPath',
    'language',
  ],
  'config.middleware': ['builtin', 'workers', 'timeoutMs'],
  'config.middleware.builtin': ['cors', 'timing', 'log', 'rate', 'headers'],
  'config.middleware.builtin.cors': ['origins', 'methods', 'headers', 'credentials', 'maxAge'],
  'config.middleware.builtin.rate': ['max', 'window', 'key'],
})

/**
 * Two keys the schema carries that `RuvyxaConfig` deliberately does not.
 *
 * Both are accepted so a config written against an older release still
 * validates, and both are read by nothing. Declaring them in the public type
 * would advertise settings that select no behaviour, so the type omits them and
 * this list is what keeps the conformance test honest about the difference.
 */
export const DEPRECATED_CONFIG_KEYS = Object.freeze(['react', 'typescript'])
