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
              'Run this command from the copied `deploy/deno/` directory. `PORT` defaults to 3000 and `HOST` to 0.0.0.0.\n\n' +
              '## Deno Deploy\n\n' +
              'Deno Deploy has no framework preset for Ruvyxa, so set the build\n' +
              'configuration yourself. Use a **dynamic** runtime and give it this\n' +
              'entrypoint, relative to the repository root:\n\n' +
              '```\n.ruvyxa/deploy/deno/server/index.mjs\n```\n\n' +
              'The build detects Deno Deploy through `DENO_DEPLOY`, so no adapter\n' +
              'needs naming in `ruvyxa.config.ts`. `PORT` is supplied by the platform.\n\n' +
              'Do not put a `deno.json` in this directory. It would make the directory\n' +
              "Deno's own configuration scope, and that scope declares no dependencies —\n" +
              'so the npm packages a server-components render reaches for at request\n' +
              'time stop resolving and every such route answers 500. Without one, Deno\n' +
              'walks up to the project and resolves them.\n',
          },
        ],
      }
    },
  }
}

export default deno
