import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, validateBuildContext } from '@ruvyxa/core'

/**
 * Options for the Vercel adapter.
 */
export interface VercelAdapterOptions {
  /** Custom functions output directory. Defaults to `${outDir}/functions`. */
  functionsDir?: string
}

/**
 * Create a Vercel deployment adapter for Ruvyxa.
 *
 * Produces serverless function bundles and static assets compatible
 * with Vercel's build output API. Generates a `vercel.json` config
 * reference for routing.
 *
 * @example
 * ```ts
 * import { config } from "ruvyxa/config"
 * import { vercelAdapter } from "@ruvyxa/adapter-vercel"
 *
 * export default config({
 *   adapter: vercelAdapter({ functionsDir: ".vercel/output/functions" })
 * })
 * ```
 */
export function vercelAdapter(options: VercelAdapterOptions = {}): Adapter {
  if (options.functionsDir !== undefined && typeof options.functionsDir !== 'string') {
    throw new Error(
      `[RUV2001] vercelAdapter: "functionsDir" must be a string, got ${typeof options.functionsDir}`,
    )
  }

  if (options.functionsDir !== undefined && options.functionsDir.trim() === '') {
    throw new Error(`[RUV2001] vercelAdapter: "functionsDir" must not be an empty string`)
  }

  return {
    name: 'vercel',
    target: 'serverless',
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'vercelAdapter')
      const functionsDir = options.functionsDir ?? `${ctx.outDir}/functions`
      return {
        name: 'vercel',
        target: 'serverless',
        platform: 'vercel',
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        functionsDir,
        configFiles: ['vercel.json'],
      }
    },
  }
}

export default vercelAdapter
