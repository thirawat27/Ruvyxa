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

/**
 * Content-Type for each extension a `public/` file may carry.
 *
 * Held to `tests/fixtures/static-asset-conformance.json` alongside
 * `content_type_for` in `crates/ruvyxa_dev_server/src/static_assets.rs`, which
 * answers the same question for `ruvyxa dev`, `start`, and `preview`. The two
 * tables were written independently and had drifted: the same file was a
 * `font/woff2` on one and an `application/octet-stream` on the other, and a
 * `.wasm` module served by the wrong one could not be streamed at all.
 *
 * Lives here rather than inside the generated server source so a test can read
 * it. It used to be a literal in the middle of the template string that
 * `standaloneServerSource` returns, which put it beyond the reach of any check.
 */
export const STATIC_CONTENT_TYPES: Readonly<Record<string, string>> = {
  apng: 'image/apng',
  avif: 'image/avif',
  bmp: 'image/bmp',
  css: 'text/css; charset=utf-8',
  eot: 'application/vnd.ms-fontobject',
  gif: 'image/gif',
  html: 'text/html; charset=utf-8',
  ico: 'image/x-icon',
  jpeg: 'image/jpeg',
  jpg: 'image/jpeg',
  js: 'text/javascript; charset=utf-8',
  json: 'application/json; charset=utf-8',
  map: 'application/json; charset=utf-8',
  mjs: 'text/javascript; charset=utf-8',
  mov: 'video/quicktime',
  mp3: 'audio/mpeg',
  mp4: 'video/mp4',
  ogg: 'audio/ogg',
  otf: 'font/otf',
  png: 'image/png',
  svg: 'image/svg+xml',
  ttf: 'font/ttf',
  txt: 'text/plain; charset=utf-8',
  wasm: 'application/wasm',
  wav: 'audio/wav',
  webm: 'video/webm',
  webmanifest: 'application/manifest+json; charset=utf-8',
  webp: 'image/webp',
  woff2: 'font/woff2',
  woff: 'font/woff',
  xml: 'application/xml; charset=utf-8',
}

/** Served when an extension has no entry, rather than guessing from content. */
export const FALLBACK_CONTENT_TYPE = 'application/octet-stream'

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
 * Largest width `/__ruvyxa/image` accepts when the build named none.
 *
 * `ruvyxa build` publishes the project's own `image.onDemand.maxWidth` into the
 * runtime policy, so this is the answer for a function bundle built before that
 * field existed — and it has to be the same number the native host uses, or the
 * same URL is a 200 under `ruvyxa start` and a 400 from the deployment.
 *
 * It is `defaultMaxWidth` in `tests/fixtures/dynamic-image-conformance.json`.
 * Two Rust declarations replay that file; the JavaScript side had three more
 * written as literals — one per adapter that can optimize — and replayed none.
 * This is the one JavaScript declaration, and
 * `tests/packages/ruvyxa/serverless-shared-tables.test.mjs` holds it and every
 * emitted function source to the fixture.
 */
export const DEFAULT_IMAGE_MAX_WIDTH = 3840

/** Non-breaking response security defaults shared by every Ruvyxa runtime. */
export const DEFAULT_SECURITY_HEADERS = {
  'X-Content-Type-Options': 'nosniff',
  'Referrer-Policy': 'strict-origin-when-cross-origin',
  'Permissions-Policy': 'camera=(), microphone=(), geolocation=()',
  'Cross-Origin-Opener-Policy': 'same-origin',
  'Cross-Origin-Resource-Policy': 'same-origin',
  'X-Frame-Options': 'DENY',
  'X-Permitted-Cross-Domain-Policies': 'none',
} as const

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
  const securityRules = Object.entries(DEFAULT_SECURITY_HEADERS)
    .map(([name, value]) => `  ${name}: ${value}\n`)
    .join('')
  const assetRules = publicAssetGlobs()
    .map((glob) => `${glob}\n  Cache-Control: ${PUBLIC_ASSET_CACHE_CONTROL}\n`)
    .join('')
  return `/*\n${securityRules}${CLIENT_BUNDLE_PREFIX}*\n  Cache-Control: ${IMMUTABLE_CACHE_CONTROL}\n${assetRules}`
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

function policySection(value: unknown): Readonly<Record<string, unknown>> | undefined {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Readonly<Record<string, unknown>>)
    : undefined
}

/**
 * Return the validated runtime policy a deployed handler needs.
 *
 * Assembled explicitly rather than by returning `buildInfo.runtime` verbatim.
 * `ruvyxa build` writes the validated `security` block as a sibling of
 * `runtime` in `build.json`, and returning only `runtime` silently dropped it:
 * every deployed runtime then ignored `security.apiLimit` (so a serverless
 * function had no request body cap at all), `security.headers: false`, and
 * `security.trustedProxyIps`, while `ruvyxa start` enforced all three. The
 * shape of `build.json` is public and read by other tooling, so the mapping
 * from build metadata to runtime policy belongs here — in one place both the
 * standalone server and every serverless adapter go through.
 */
export function runtimeBuildPolicy(ctx: BuildContext): Readonly<Record<string, unknown>> {
  const runtime = policySection(ctx.buildInfo?.runtime) ?? {}
  const security = policySection(ctx.buildInfo?.security)
  return security ? { ...runtime, security } : runtime
}

/**
 * Characters an `outDir` may contain when it is going into a generated command.
 *
 * Deliberately wide: this is not a security boundary — `outDir` is the
 * project's own configuration, not request input — so the only job is to refuse
 * the values that produce a *deployment* that fails rather than a build that
 * does.
 */
const SAFE_OUT_DIR = /^[A-Za-z0-9._/-]+$/

/**
 * Refuse an `outDir` that cannot be interpolated into a generated deployment.
 *
 * `adapter-render` writes its blueprint by string concatenation
 * (`'    startCommand: node ' + serverEntry`) and `adapter-railway` interpolates
 * the same path into a `startCommand` string. Neither quoted it, and the
 * characters come from `ruvyxa.config.ts`:
 *
 * - `#` starts a YAML comment and truncates the command;
 * - `: ` turns the scalar into a nested mapping and fails the parse;
 * - a space produces valid YAML and an invalid shell command.
 *
 * The failure then lands on the platform rather than on the developer's
 * machine, which is the expensive place to find it — so it is refused at build
 * time, naming the setting. `adapter-static` already validated its equivalent
 * input this way; this is the same refusal for the two that did not.
 */
export function assertSafeOutDirForCommand(adapter: string, relativeOutDir: string): void {
  if (!SAFE_OUT_DIR.test(relativeOutDir)) {
    throw new Error(
      `[RUV2001] ${adapter}: "outDir" must contain only letters, digits, and \`.\`, \`_\`, ` +
        `\`-\` or \`/\` to be used in a generated start command; got "${relativeOutDir}"`,
    )
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
  // Written as a scan rather than `/\/+$/`, which the engine retries from every
  // start position: a path ending in many separators costs time quadratic in
  // its length. `outDir` is configuration rather than request input, so this was
  // never reachable from outside — but the linear form is the same three lines
  // and removes the question.
  const normalize = (value: string) => {
    const slashed = value.replaceAll('\\', '/')
    let end = slashed.length
    while (end > 0 && slashed[end - 1] === '/') end -= 1
    return slashed.slice(0, end)
  }
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
