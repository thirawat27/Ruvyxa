import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, validateBuildContext } from '@ruvyxa/core'

/**
 * Options for the static site adapter.
 */
export interface StaticAdapterOptions {
  /** Custom output directory for the static site. Defaults to `${outDir}/static`. */
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
 *   adapter: staticAdapter({ outputDir: "./public" })
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

  return {
    name: 'static',
    target: 'static',
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'staticAdapter')
      return {
        name: 'static',
        target: 'static',
        platform: 'static',
        entry: options.outputDir ?? `${ctx.outDir}/static`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
      }
    },
  }
}

export default staticAdapter
