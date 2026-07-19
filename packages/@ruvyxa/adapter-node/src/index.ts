import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, validateBuildContext } from '@ruvyxa/core'

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
 * import { config } from "ruvyxa/config"
 * import { nodeAdapter } from "@ruvyxa/adapter-node"
 *
 * export default config({
 *   adapter: nodeAdapter({ entry: "./custom-entry" })
 * })
 * ```
 */
export function nodeAdapter(options: NodeAdapterOptions = {}): Adapter {
  if (options.entry !== undefined && typeof options.entry !== 'string') {
    throw new Error(`[RUV2001] nodeAdapter: "entry" must be a string, got ${typeof options.entry}`)
  }

  if (options.entry !== undefined && options.entry.trim() === '') {
    throw new Error(`[RUV2001] nodeAdapter: "entry" must not be an empty string`)
  }

  return {
    name: 'node',
    target: 'node',
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'nodeAdapter')
      return {
        name: 'node',
        target: 'node',
        platform: 'node',
        entry: options.entry ?? `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        artifacts: [
          {
            kind: 'file',
            path: 'deploy/node/start.mjs',
            contents: `import { spawn } from 'node:child_process'\n\nconst child = spawn('npx', ['--no-install', 'ruvyxa', 'start'], { cwd: process.cwd(), stdio: 'inherit' })\nchild.on('exit', (code, signal) => process.exitCode = code ?? (signal ? 1 : 0))\n`,
          },
          {
            kind: 'file',
            path: 'deploy/node/README.md',
            contents:
              '# Ruvyxa Node deployment\\n\\nRun `node .ruvyxa/deploy/node/start.mjs` from the application root after installing production dependencies.\\n',
          },
        ],
      }
    },
  }
}

export default nodeAdapter
