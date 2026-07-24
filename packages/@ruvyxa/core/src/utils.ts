import type { BuildContext } from './types.js'

/**
 * Extensions that only ever name a build or public asset.
 *
 * Restricted to images, fonts, media, and emitted web assets: none of these is
 * a plausible value for a dynamic route parameter, so a host rule keyed on
 * them cannot swallow a real page. Kept in sync with `STATIC_ASSET_EXTENSIONS`
 * in `packages/ruvyxa/runtime/serverless-handler.mjs` and
 * `crates/ruvyxa_dev_server/src/static_assets.rs`.
 */
export const STATIC_ASSET_EXTENSIONS = [
  'apng',
  'avif',
  'bmp',
  'css',
  'eot',
  'gif',
  'ico',
  'jpeg',
  'jpg',
  'js',
  'map',
  'mjs',
  'mov',
  'mp3',
  'mp4',
  'ogg',
  'otf',
  'png',
  'svg',
  'ttf',
  'wav',
  'webm',
  'webp',
  'woff',
  'woff2',
] as const

/**
 * PCRE pattern matching a public asset URL, used by host routing tables.
 *
 * `/__ruvyxa/` is excluded because hashed client bundles carry their own
 * immutable caching rule; letting this pattern match them too would overwrite
 * that header with the shorter public-asset lifetime.
 */
export function staticAssetPattern(): string {
  return `^/(?!__ruvyxa/).+\\.(?:${STATIC_ASSET_EXTENSIONS.join('|')})$`
}

/** Glob list for the same assets, for hosts whose config takes paths, not regexes. */
export function staticAssetGlobs(): string[] {
  return STATIC_ASSET_EXTENSIONS.map((extension) => `/*.${extension}`)
}

/** URL prefix of the content-hashed client bundles. */
export const CLIENT_BUNDLE_PREFIX = '/__ruvyxa/client/'

/** Cache policy for content-hashed bundles: the URL changes when the bytes do. */
export const IMMUTABLE_CACHE_CONTROL = 'public, max-age=31536000, immutable'

/**
 * Cache policy for `public/` assets, which are not content-hashed.
 *
 * Identical to the header `serve_public_file` sends in `ruvyxa dev` and
 * `ruvyxa start`, so a file behaves the same locally and on a CDN. Without it
 * Vercel, Netlify, and Cloudflare all default to `max-age=0, must-revalidate`
 * and every navigation re-fetches each image and font.
 */
export const PUBLIC_ASSET_CACHE_CONTROL = 'public, max-age=3600, must-revalidate'

/**
 * Glob list for `_headers`-style host config: images, fonts, and media only.
 *
 * Deliberately excludes `css`/`js`/`mjs`/`map`. On hosts whose `*` matches
 * across path separators, a `/*.js` rule would also match
 * `/__ruvyxa/client/<hash>.js` and replace its immutable header with this much
 * shorter lifetime. Vercel's rule keeps those extensions because its pattern
 * excludes the client prefix explicitly.
 */
export function publicAssetGlobs(): string[] {
  const emitted = new Set(['css', 'js', 'map', 'mjs'])
  return STATIC_ASSET_EXTENSIONS.filter((extension) => !emitted.has(extension)).map(
    (extension) => `/*.${extension}`,
  )
}

/** `_headers` file contents shared by every host that reads one. */
export function headersFileContents(): string {
  const assetRules = publicAssetGlobs()
    .map((glob) => `${glob}\n  Cache-Control: ${PUBLIC_ASSET_CACHE_CONTROL}\n`)
    .join('')
  return `${CLIENT_BUNDLE_PREFIX}*\n  Cache-Control: ${IMMUTABLE_CACHE_CONTROL}\n${assetRules}`
}

/** Return the standard client bundle paths consumed by deployment adapters. */
export function clientBuildOutput(ctx: BuildContext): {
  clientDir: string
  chunkManifest: string
} {
  return {
    clientDir: `${ctx.outDir}/client`,
    chunkManifest: ctx.chunkManifest ?? `${ctx.outDir}/client/chunk-manifest.json`,
  }
}

/**
 * Return `ctx.outDir` as a project-root-relative POSIX path.
 *
 * Adapter-generated config files (netlify.toml, wrangler.jsonc) are read on
 * other machines and other operating systems, so they must never embed the
 * absolute build-machine path that `BuildContext.outDir` carries. Windows
 * separators are normalized to `/` and a trailing separator is dropped. When
 * `outDir` does not live under `root` (already relative, or a custom
 * out-of-tree directory), the normalized value is returned unchanged.
 */
export function projectRelativeOutDir(ctx: BuildContext): string {
  const normalize = (value: string) => value.replace(/\\/g, '/').replace(/\/+$/, '')
  const root = normalize(ctx.root)
  const outDir = normalize(ctx.outDir)
  if (root !== '' && outDir.startsWith(`${root}/`)) {
    return outDir.slice(root.length + 1)
  }
  return outDir
}

export function validateBuildContext(
  ctx: BuildContext,
  adapterName: string,
): asserts ctx is BuildContext {
  if (!ctx.root || typeof ctx.root !== 'string') {
    throw new Error(
      `[RUV2000] ${adapterName}: BuildContext.root is required and must be a non-empty string`,
    )
  }
  if (!ctx.outDir || typeof ctx.outDir !== 'string') {
    throw new Error(
      `[RUV2000] ${adapterName}: BuildContext.outDir is required and must be a non-empty string`,
    )
  }
}
