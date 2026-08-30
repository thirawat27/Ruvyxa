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
 * Pre-renders all pages to static HTML files suitable for deployment on
 * any static hosting service (GitHub Pages, S3, Netlify CDN, etc.).
 * No server runtime is required.
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
  if (
    [
      'assets',
      'build.json',
      'cache',
      'client',
      'client-report.json',
      'manifest.json',
      'prerender',
      'server',
    ].includes(segments[0])
  ) {
    throw new Error(
      '[RUV2001] staticAdapter: "outputDir" overlaps protected build output; use a directory such as static or deploy/public',
    )
  }
  return normalized
}

export { createStatic as static }
export default createStatic
