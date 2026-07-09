import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { validateBuildContext } from '@ruvyxa/core'

/**
 * Options for the Netlify adapter.
 */
export interface NetlifyAdapterOptions {
  /** Custom Netlify functions directory. Defaults to `${outDir}/netlify/functions`. */
  functionsDir?: string
}

/**
 * Create a Netlify deployment adapter for Ruvyxa.
 *
 * Produces serverless function bundles and static assets for Netlify
 * deployment. Generates a `netlify.toml` config reference for routing.
 *
 * @example
 * ```ts
 * import { defineConfig } from "ruvyxa/config"
 * import { netlifyAdapter } from "@ruvyxa/adapter-netlify"
 *
 * export default defineConfig({
 *   adapter: netlifyAdapter({ functionsDir: "netlify/functions" })
 * })
 * ```
 */
export function netlifyAdapter(options: NetlifyAdapterOptions = {}): Adapter {
  if (options.functionsDir !== undefined && typeof options.functionsDir !== 'string') {
    throw new Error(
      `[RUV2001] netlifyAdapter: "functionsDir" must be a string, got ${typeof options.functionsDir}`,
    )
  }

  if (options.functionsDir !== undefined && options.functionsDir.trim() === '') {
    throw new Error(`[RUV2001] netlifyAdapter: "functionsDir" must not be an empty string`)
  }

  return {
    name: 'netlify',
    target: 'serverless',
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'netlifyAdapter')
      const functionsDir = options.functionsDir ?? `${ctx.outDir}/netlify/functions`
      return {
        name: 'netlify',
        target: 'serverless',
        platform: 'netlify',
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        functionsDir,
        configFiles: ['netlify.toml'],
      }
    },
  }
}

export default netlifyAdapter
