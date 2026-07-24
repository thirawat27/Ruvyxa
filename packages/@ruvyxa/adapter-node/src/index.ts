import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, standaloneServerSource, validateBuildContext } from '@ruvyxa/core'

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
 * Produces a self-contained standalone server in `deploy/node/`:
 * `server/index.mjs` (a plain `node:http` server around the generic
 * serverless handler) plus a `public/` directory with pre-rendered pages and
 * hashed client bundles. Runs on any Node.js hosting (Docker, PM2, systemd,
 * any PaaS) with `node server/index.mjs` — no ruvyxa CLI or native binary is
 * needed at runtime. Honors `PORT` and `HOST`.
 *
 * @example
 * ```ts
 * import { config } from "ruvyxa/config"
 * import { nodeAdapter } from "@ruvyxa/adapter-node"
 *
 * export default config({
 *   adapter: nodeAdapter()
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
    supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
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
          // Standalone server: compiled route registry + handler runtime
          {
            kind: 'function',
            path: 'deploy/node/server',
            handlerSource: standaloneServerSource(),
          },
          // Static publish directory served by the standalone server. An
          // API-only app has no prerendered pages; the server still runs.
          { kind: 'static-site', path: 'deploy/node/public', optional: true },
          {
            kind: 'file',
            path: 'deploy/node/start.mjs',
            // npx resolves to npx.cmd on Windows, which spawn() refuses to
            // execute without a shell; keep the shell off elsewhere.
            contents: `import { spawn } from 'node:child_process'\n\nconst child = spawn('npx', ['--no-install', 'ruvyxa', 'start'], { cwd: process.cwd(), stdio: 'inherit', shell: process.platform === 'win32' })\nchild.on('exit', (code, signal) => process.exitCode = code ?? (signal ? 1 : 0))\n`,
          },
          {
            kind: 'file',
            path: 'deploy/node/README.md',
            contents:
              '# Ruvyxa Node deployment\n\n' +
              'Standalone (no ruvyxa runtime dependency):\n\n' +
              '```bash\nnode .ruvyxa/deploy/node/server/index.mjs\n```\n\n' +
              'Honors `PORT` (default 3000) and `HOST` (default 0.0.0.0). Copy the\n' +
              '`deploy/node/` directory anywhere — Docker, PM2, systemd, any PaaS —\n' +
              'and run the same command.\n\n' +
              'Alternative, using the installed ruvyxa CLI:\n\n' +
              '```bash\nnode .ruvyxa/deploy/node/start.mjs\n```\n',
          },
        ],
      }
    },
  }
}

export default nodeAdapter
