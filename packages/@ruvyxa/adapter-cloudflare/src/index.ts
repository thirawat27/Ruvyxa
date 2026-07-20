import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, validateBuildContext } from '@ruvyxa/core'

/**
 * Options for the Cloudflare static deployment adapter.
 */
export interface CloudflareAdapterOptions {
  /** Custom worker entry point path. Defaults to `${outDir}/server/app`. */
  workerEntry?: string
}

/**
 * Create a Cloudflare Pages static deployment adapter for Ruvyxa.
 *
 * Produces static assets ready for deployment via `wrangler` and generates a
 * `wrangler.jsonc` config reference. Dynamic routes are rejected at build time.
 *
 * @example
 * ```ts
 * import { config } from "ruvyxa/config"
 * import { cloudflareAdapter } from "@ruvyxa/adapter-cloudflare"
 *
 * export default config({
 *   adapter: cloudflareAdapter({ workerEntry: "./src/worker.ts" })
 * })
 * ```
 */
export function cloudflareAdapter(options: CloudflareAdapterOptions = {}): Adapter {
  if (options.workerEntry !== undefined && typeof options.workerEntry !== 'string') {
    throw new Error(
      `[RUV2001] cloudflareAdapter: "workerEntry" must be a string, got ${typeof options.workerEntry}`,
    )
  }

  if (options.workerEntry !== undefined && options.workerEntry.trim() === '') {
    throw new Error(`[RUV2001] cloudflareAdapter: "workerEntry" must not be an empty string`)
  }

  return {
    name: 'cloudflare',
    target: 'edge',
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'cloudflareAdapter')
      return {
        name: 'cloudflare',
        target: 'edge',
        platform: 'cloudflare',
        entry: options.workerEntry ?? `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        configFiles: ['wrangler.jsonc'],
        artifacts: [
          { kind: 'static-site', path: 'deploy/cloudflare/assets' },
          {
            kind: 'file',
            path: 'deploy/cloudflare/wrangler.jsonc',
            contents: '{\n  "name": "ruvyxa-app",\n  "assets": { "directory": "./assets" }\n}\n',
          },
          {
            // Workers static assets read _headers from the asset directory;
            // hashed client bundles are immutable.
            kind: 'file',
            path: 'deploy/cloudflare/assets/_headers',
            contents: '/client/*\n  Cache-Control: public, max-age=31536000, immutable\n',
          },
        ],
      }
    },
  }
}

export default cloudflareAdapter
