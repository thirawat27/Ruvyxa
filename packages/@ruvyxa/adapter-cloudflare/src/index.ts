import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { validateBuildContext } from '@ruvyxa/core'

/**
 * Options for the Cloudflare Workers adapter.
 */
export interface CloudflareAdapterOptions {
  /** Custom worker entry point path. Defaults to `${outDir}/server/app`. */
  workerEntry?: string
}

/**
 * Create a Cloudflare Workers deployment adapter for Ruvyxa.
 *
 * Produces an edge-compatible worker bundle and static assets ready for
 * deployment via `wrangler`. Generates a `wrangler.toml` config reference.
 *
 * @example
 * ```ts
 * import { defineConfig } from "ruvyxa/config"
 * import { cloudflareAdapter } from "@ruvyxa/adapter-cloudflare"
 *
 * export default defineConfig({
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
        configFiles: ['wrangler.toml'],
      }
    },
  }
}

export default cloudflareAdapter
