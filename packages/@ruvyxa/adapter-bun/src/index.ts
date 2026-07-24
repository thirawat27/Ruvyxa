import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, standaloneServerSource, validateBuildContext } from '@ruvyxa/core'

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
 * Produces the same self-contained deployment the node adapter does — a
 * compiled route registry, the request handler, and a `public/` directory —
 * and runs it with `bun .ruvyxa/deploy/bun/server/index.mjs`. Bun implements
 * `node:http`, `import.meta.dirname`, and the Web `Request`/`Response` classes
 * the handler is written against, so the server source is shared rather than
 * forked.
 *
 * Earlier releases emitted only a launcher that shelled out to
 * `bunx ruvyxa start`, which meant a Bun host still needed the ruvyxa CLI and
 * its native binary installed at runtime. The launcher is still emitted for
 * that workflow; it is no longer the only option.
 *
 * @example
 * ```ts
 * import { config } from "ruvyxa/config"
 * import { bunAdapter } from "@ruvyxa/adapter-bun"
 *
 * export default config({
 *   adapter: bunAdapter()
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
    supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
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
          // Standalone server: compiled route registry + handler runtime
          {
            kind: 'function',
            path: 'deploy/bun/server',
            handlerSource: standaloneServerSource(),
          },
          // Static publish directory served by the standalone server. An
          // API-only app has no prerendered pages; the server still runs.
          { kind: 'static-site', path: 'deploy/bun/public', optional: true },
          {
            kind: 'file',
            path: 'deploy/bun/start.mjs',
            contents: `const child = Bun.spawn(['bunx', '--no-install', 'ruvyxa', 'start'], { cwd: process.cwd(), stdin: 'inherit', stdout: 'inherit', stderr: 'inherit' })\nprocess.exitCode = await child.exited\n`,
          },
          {
            kind: 'file',
            path: 'deploy/bun/README.md',
            contents:
              '# Ruvyxa Bun deployment\n\n' +
              'Standalone (no ruvyxa runtime dependency):\n\n' +
              '```bash\nbun .ruvyxa/deploy/bun/server/index.mjs\n```\n\n' +
              'Honors `PORT` (default 3000) and `HOST` (default 0.0.0.0). Copy the\n' +
              '`deploy/bun/` directory anywhere Bun runs and use the same command.\n\n' +
              'Alternative, using the installed ruvyxa CLI:\n\n' +
              '```bash\nbun .ruvyxa/deploy/bun/start.mjs\n```\n',
          },
        ],
      }
    },
  }
}

export default bunAdapter
