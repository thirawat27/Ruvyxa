import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, validateBuildContext } from '@ruvyxa/core'

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
 * import { staticAdapter } from "@ruvyxa/adapter-static"
 *
 * export default config({
 *   adapter: staticAdapter({ outputDir: "public" })
 * })
 * ```
 */
export function staticAdapter(options: StaticAdapterOptions = {}): Adapter {
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
        artifacts: [{ kind: 'static-site', path: outputDir }],
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
  if (
    ['assets', 'build.json', 'cache', 'client', 'manifest.json', 'prerender', 'server'].includes(
      segments[0],
    )
  ) {
    throw new Error(
      '[RUV2001] staticAdapter: "outputDir" overlaps protected build output; use a directory such as static or deploy/public',
    )
  }
  return normalized
}

export default staticAdapter
