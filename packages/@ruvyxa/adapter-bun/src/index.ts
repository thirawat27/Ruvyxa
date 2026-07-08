import type { Adapter, AdapterOutput, BuildContext } from "@ruvyxa/core"
import { validateBuildContext } from "@ruvyxa/core"

/**
 * Options for the Bun adapter.
 */
export interface BunAdapterOptions {
  /** Custom entry point path. Defaults to `${outDir}/server/app`. */
  entry?: string
}

/**
 * Create a Bun runtime deployment adapter for Ruvyxa.
 *
 * Produces a Bun-optimized server bundle that takes advantage of Bun's
 * native performance features. Deploys to any Bun-compatible hosting.
 *
 * @example
 * ```ts
 * import { defineConfig } from "ruvyxa/config"
 * import { bunAdapter } from "@ruvyxa/adapter-bun"
 *
 * export default defineConfig({
 *   adapter: bunAdapter({ entry: "./bun-entry.ts" })
 * })
 * ```
 */
export function bunAdapter(options: BunAdapterOptions = {}): Adapter {
  if (options.entry !== undefined && typeof options.entry !== "string") {
    throw new Error(
      `[RUV2001] bunAdapter: "entry" must be a string, got ${typeof options.entry}`,
    )
  }

  if (options.entry !== undefined && options.entry.trim() === "") {
    throw new Error(
      `[RUV2001] bunAdapter: "entry" must not be an empty string`,
    )
  }

  return {
    name: "bun",
    target: "node",
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, "bunAdapter")
      return {
        name: "bun",
        target: "node",
        platform: "bun",
        entry: options.entry ?? `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
      }
    },
  }
}

export default bunAdapter
