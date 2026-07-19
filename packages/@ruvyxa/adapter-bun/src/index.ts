import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, validateBuildContext } from '@ruvyxa/core'

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
 * import { config } from "ruvyxa/config"
 * import { bunAdapter } from "@ruvyxa/adapter-bun"
 *
 * export default config({
 *   adapter: bunAdapter({ entry: "./bun-entry.ts" })
 * })
 * ```
 */
export function bunAdapter(options: BunAdapterOptions = {}): Adapter {
  if (options.entry !== undefined && typeof options.entry !== 'string') {
    throw new Error(`[RUV2001] bunAdapter: "entry" must be a string, got ${typeof options.entry}`)
  }

  if (options.entry !== undefined && options.entry.trim() === '') {
    throw new Error(`[RUV2001] bunAdapter: "entry" must not be an empty string`)
  }

  return {
    name: 'bun',
    target: 'node',
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'bunAdapter')
      return {
        name: 'bun',
        target: 'node',
        platform: 'bun',
        runtime: 'bun',
        entry: options.entry ?? `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        artifacts: [
          {
            kind: 'file',
            path: 'deploy/bun/start.mjs',
            contents: `const child = Bun.spawn(['bunx', '--no-install', 'ruvyxa', 'start'], { cwd: process.cwd(), stdin: 'inherit', stdout: 'inherit', stderr: 'inherit' })\nprocess.exitCode = await child.exited\n`,
          },
          {
            kind: 'file',
            path: 'deploy/bun/README.md',
            contents:
              '# Ruvyxa Bun deployment\\n\\nRun `bun .ruvyxa/deploy/bun/start.mjs` from the application root after installing production dependencies.\\n',
          },
        ],
      }
    },
  }
}

export default bunAdapter
