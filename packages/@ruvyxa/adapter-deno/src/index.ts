import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import {
  clientBuildOutput,
  runtimeBuildPolicy,
  standaloneServerSource,
  validateBuildContext,
} from '@ruvyxa/core'

export interface DenoAdapterOptions {
  /** Custom entry point path. Defaults to `${outDir}/server/app`. */
  entry?: string
}

/** Create a self-contained Deno deployment for a Ruvyxa application. */
export function deno(options: DenoAdapterOptions = {}): Adapter {
  if (options.entry !== undefined && typeof options.entry !== 'string') {
    throw new Error(`[RUV2001] denoAdapter: "entry" must be a string, got ${typeof options.entry}`)
  }
  if (options.entry !== undefined && options.entry.trim() === '') {
    throw new Error('[RUV2001] denoAdapter: "entry" must not be an empty string')
  }

  return {
    name: 'deno',
    target: 'node',
    supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'denoAdapter')
      return {
        name: 'deno',
        target: 'node',
        platform: 'deno',
        runtime: 'deno',
        entry: options.entry ?? `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        artifacts: [
          {
            kind: 'function',
            path: 'deploy/deno/server',
            handlerSource: standaloneServerSource({
              runtime: 'deno',
              runtimePolicy: runtimeBuildPolicy(ctx),
            }),
          },
          { kind: 'static-site', path: 'deploy/deno/public', optional: true },
          {
            kind: 'file',
            path: 'deploy/deno/start.mjs',
            contents:
              `const child = new Deno.Command(Deno.execPath(), { args: ['run', '-A', '--no-prompt', 'npm:ruvyxa', 'start'], cwd: Deno.cwd(), stdin: 'inherit', stdout: 'inherit', stderr: 'inherit' }).spawn()\n` +
              `const status = await child.status\nDeno.exit(status.code)\n`,
          },
          {
            kind: 'file',
            path: 'deploy/deno/README.md',
            contents:
              '# Ruvyxa Deno deployment\n\n' +
              'Run the self-contained server (no Ruvyxa CLI dependency):\n\n' +
              '```bash\ndeno run -A --no-prompt server/index.mjs\n```\n\n' +
              'Run this command from the copied `deploy/deno/` directory. `PORT` defaults to 3000 and `HOST` to 0.0.0.0.\n',
          },
        ],
      }
    },
  }
}

export default deno
