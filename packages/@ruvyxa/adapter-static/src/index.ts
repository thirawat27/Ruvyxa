import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, headersFileContents, validateBuildContext } from '@ruvyxa/core'

/**
 * Options for the static site adapter.
 */
export interface StaticAdapterOptions {
  /** Directory under Ruvyxa's build output. Defaults to `static`. */
  outputDir?: string
}

/**
 * Create a static site deployment adapter for Ruvyxa.
 *
 * Pre-renders all pages to static HTML files suitable for deployment on any
 * static hosting service (GitHub Pages, S3, Netlify CDN, etc.). No server
 * runtime is required.
 *
 * ## Response headers are not part of the output everywhere
 *
 * The only header mechanism this adapter emits is a `_headers` file, which
 * Netlify and Cloudflare Pages read and other hosts ignore. That file's own
 * comment says hosts which ignore it "are unaffected by its presence", and that
 * is true about the *file* — it is not true about the *deployment*. Every page
 * this adapter produces is a CDN-served pre-rendered document, so `createHandler`
 * never runs and there is no second place the security headers could come from.
 *
 * On GitHub Pages, S3, or any other host without a `_headers` reader, the
 * deployed site therefore ships no `X-Frame-Options`, no `X-Content-Type-Options`
 * and no `Content-Security-Policy`, and re-fetches every asset on every
 * navigation. Those hosts expect headers to be configured at the CDN or bucket,
 * which is a reasonable division — but it is worth saying plainly here, because
 * this is the one adapter whose entire output is CDN-served and therefore the
 * one where the gap covers every response rather than some of them.
 *
 * @example
 * ```ts
 * import { config } from "ruvyxa/config"
 * import { static as staticOutput } from "@ruvyxa/adapter-static"
 *
 * export default config({
 *   adapter: staticOutput({ outputDir: "public" })
 * })
 * ```
 */
function createStatic(options: StaticAdapterOptions = {}): Adapter {
  if (options.outputDir !== undefined && typeof options.outputDir !== 'string') {
    throw new Error(
      `[RUV2001] staticAdapter: "outputDir" must be a string, got ${typeof options.outputDir}`,
    )
  }

  if (options.outputDir !== undefined && options.outputDir.trim() === '') {
    throw new Error(`[RUV2001] staticAdapter: "outputDir" must not be an empty string`)
  }

  const outputDir = normalizeOutputDir(options.outputDir)

  return {
    name: 'static',
    target: 'static',
    // A static publish directory has no server, so only routes that are fully
    // materialized at build time can be deployed. Declaring this lets the
    // adapter runner reject SSR/ISR/PPR pages and API routes with a per-route
    // error before the build hook runs.
    supports: ['ssg', 'csr'],
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'staticAdapter')
      return {
        name: 'static',
        target: 'static',
        platform: 'static',
        entry: `${ctx.outDir}/${outputDir}`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        artifacts: [
          { kind: 'static-site', path: outputDir },
          {
            // Hosts that read `_headers` from the publish root (Netlify,
            // Cloudflare Pages) otherwise serve even the content-hashed client
            // bundles with a revalidate-every-time default. Hosts that ignore
            // the file are unaffected by its presence.
            kind: 'file',
            path: `${outputDir}/_headers`,
            contents: headersFileContents(),
          },
        ],
      }
    },
  }
}

/**
 * Build output an `outputDir` may not be spelled after.
 *
 * Not the same set as the build's own `BUILD_OUTPUT_DIRS` and
 * `BUILD_OUTPUT_FILES` in `crates/ruvyxa_cli/src/build.rs`, and deliberately so
 * — which is why it is written out here rather than derived. It differs in two
 * directions, and both matter:
 *
 * - `cache` is protected here and is not in either Rust list. It is still a
 *   directory the build writes into, and a static site written over it destroys
 *   the compile cache.
 * - `deploy` and `static` are in the Rust list and are *not* protected here.
 *   They are where adapter output is supposed to go, and this adapter's own
 *   error message tells the author to use one of them.
 *
 * `keeps-step-with-the-build-output` in this package's tests holds the
 * relationship, so a directory added to the build cannot quietly become
 * writable from here.
 */
const PROTECTED_BUILD_OUTPUT = new Set([
  'assets',
  'build.json',
  'cache',
  'client',
  'client-report.json',
  'manifest.json',
  'prerender',
  'server',
])

function normalizeOutputDir(value: string | undefined): string {
  const normalized = (value ?? 'static').trim().replaceAll('\\', '/')
  const segments = normalized.split('/')
  if (
    normalized.startsWith('/') ||
    /^[A-Za-z]:/.test(normalized) ||
    segments.some((segment) => segment === '' || segment === '.' || segment === '..')
  ) {
    throw new Error(
      '[RUV2001] staticAdapter: "outputDir" must be a non-empty relative directory inside the build output',
    )
  }
  // Names the build owns at its root. `client-report.json` is the client build
  // report; it used to be `client/manifest.json`, covered by `client`, and now
  // sits outside the published directory precisely so it is never served — but
  // that also took it out from behind `client`, and an `outputDir` spelled
  // after it would write the static site over the file the pre-renderer and
  // every adapter function read.
  if (PROTECTED_BUILD_OUTPUT.has(segments[0])) {
    throw new Error(
      '[RUV2001] staticAdapter: "outputDir" overlaps protected build output; use a directory such as static or deploy/public',
    )
  }
  return normalized
}

export { createStatic as static }
export default createStatic
