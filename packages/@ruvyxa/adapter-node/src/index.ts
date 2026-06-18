import type { Adapter, AdapterOutput, BuildContext } from "@ruvyxa/core"

/**
 * Options for the Node.js adapter.
 */
export interface NodeAdapterOptions {
  /** Custom entry point path. Defaults to `${outDir}/server/app`. */
  entry?: string
}

/**
 * Create a Node.js deployment adapter for Ruvyxa.
 *
 * Produces a standard Node.js server bundle suitable for deployment
 * on any Node.js hosting (Docker, PM2, systemd, etc.).
 *
 * @example
 * ```ts
 * import { defineConfig } from "ruvyxa/config"
 * import { nodeAdapter } from "@ruvyxa/adapter-node"
 *
 * export default defineConfig({
 *   adapter: nodeAdapter({ entry: "./custom-entry" })
 * })
 * ```
 */
export function nodeAdapter(options: NodeAdapterOptions = {}): Adapter {
  if (options.entry !== undefined && typeof options.entry !== "string") {
    throw new Error(
      `[RUV2001] nodeAdapter: "entry" must be a string, got ${typeof options.entry}`,
    )
  }

  if (options.entry !== undefined && options.entry.trim() === "") {
    throw new Error(
      `[RUV2001] nodeAdapter: "entry" must not be an empty string`,
    )
  }

  return {
    name: "node",
    target: "node",
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, "nodeAdapter")
      return {
        name: "node",
        target: "node",
        platform: "node",
        entry: options.entry ?? `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
      }
    },
  }
}

export default nodeAdapter

// --- Shared Validation ---
function validateBuildContext(ctx: BuildContext, adapterName: string): void {
  if (!ctx.root || typeof ctx.root !== "string") {
    throw new Error(
      `[RUV2000] ${adapterName}: BuildContext.root is required and must be a non-empty string`,
    )
  }
  if (!ctx.outDir || typeof ctx.outDir !== "string") {
    throw new Error(
      `[RUV2000] ${adapterName}: BuildContext.outDir is required and must be a non-empty string`,
    )
  }
}
