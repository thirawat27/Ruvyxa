import type { Adapter, AdapterArtifact, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, validateBuildContext } from '@ruvyxa/core'

/**
 * Options for Netlify metadata compatibility.
 */
export interface NetlifyAdapterOptions {
  /** Custom Netlify functions directory. Defaults to `${outDir}/netlify/functions`. */
  functionsDir?: string
  /**
   * Also emit a `netlify.toml` at the project root pointing Netlify at the
   * generated publish directory, so a fresh site deploys without dashboard
   * configuration. An existing project `netlify.toml` is never overwritten.
   * @default true
   */
  projectConfig?: boolean
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
            // Hashed client bundles are immutable; unhashed assets keep
            // Netlify's default caching.
            contents:
              '[build]\n  publish = "publish"\n\n' +
              '[[headers]]\n  for = "/client/*"\n  [headers.values]\n' +
              '    Cache-Control = "public, max-age=31536000, immutable"\n',
          },
          ...(options.projectConfig === false
            ? []
            : [
                {
                  kind: 'file',
                  path: 'netlify.toml',
                  scope: 'project',
                  skipIfExists: true,
                  contents:
                    '[build]\n' +
                    '  command = "npx --no-install ruvyxa build"\n' +
                    '  publish = ".ruvyxa/deploy/netlify/publish"\n\n' +
                    '[[headers]]\n  for = "/client/*"\n  [headers.values]\n' +
                    '    Cache-Control = "public, max-age=31536000, immutable"\n',
                } satisfies AdapterArtifact,
              ]),
        ],
      }
    },
  }
}

export default netlifyAdapter
