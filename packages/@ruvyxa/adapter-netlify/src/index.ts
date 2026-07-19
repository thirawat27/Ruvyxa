import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, validateBuildContext } from '@ruvyxa/core'

/**
 * Options for Netlify metadata compatibility.
 */
export interface NetlifyAdapterOptions {
  /** Custom Netlify functions directory. Defaults to `${outDir}/netlify/functions`. */
  functionsDir?: string
}

/**
 * Create a Netlify static deployment adapter for Ruvyxa.
 *
 * Produces static assets and a `netlify.toml` config reference. Dynamic
 * serverless function output is rejected until a Netlify request handler exists.
 *
 * @example
 * ```ts
 * import { config } from "ruvyxa/config"
 * import { netlifyAdapter } from "@ruvyxa/adapter-netlify"
 *
 * export default config({
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
        ...clientBuildOutput(ctx),
        functionsDir,
        configFiles: ['netlify.toml'],
        artifacts: [
          { kind: 'static-site', path: 'deploy/netlify/publish' },
          {
            kind: 'file',
            path: 'deploy/netlify/netlify.toml',
            contents: '[build]\n  publish = "publish"\n',
          },
        ],
      }
    },
  }
}

export default netlifyAdapter
