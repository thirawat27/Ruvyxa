import type { Adapter, AdapterArtifact, AdapterOutput, BuildContext } from '@ruvyxa/core'
import {
  clientBuildOutput,
  assertSafeOutDirForCommand,
  projectRelativeOutDir,
  runtimeBuildPolicy,
  standaloneServerSource,
  validateBuildContext,
} from '@ruvyxa/core'

/** Options for Render deployments. */
export interface RenderAdapterOptions {
  /** Render Blueprint service name. @default "ruvyxa-app" */
  serviceName?: string
  /**
   * Emit a project-root `render.yaml`. Existing configuration is never overwritten.
   * @default true
   */
  projectConfig?: boolean
}

/** Create a zero-config Render web-service adapter for Ruvyxa. */
export function render(options: RenderAdapterOptions = {}): Adapter {
  const serviceName = options.serviceName ?? 'ruvyxa-app'
  if (!/^[a-z0-9](?:[a-z0-9-]{0,98}[a-z0-9])?$/.test(serviceName)) {
    throw new Error(
      '[RUV2001] renderAdapter: "serviceName" must contain lowercase letters, digits, or hyphens',
    )
  }

  return {
    name: 'render',
    target: 'node',
    supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'renderAdapter')

      // Render runs the start command from the repository root, so the path
      // has to be written relative to it — and derived from this build's
      // `outDir` rather than the `.ruvyxa` default, which a project that
      // configures `outDir` does not have.
      const relativeOutDir = projectRelativeOutDir(ctx)
      assertSafeOutDirForCommand('renderAdapter', relativeOutDir)
      const serverEntry = `${relativeOutDir}/deploy/render/server/index.mjs`

      const renderBlueprint =
        'services:\n' +
        '  - type: web\n' +
        `    name: ${JSON.stringify(serviceName)}\n` +
        '    runtime: node\n' +
        '    plan: free\n' +
        '    buildCommand: npm run build\n' +
        '    startCommand: node ' +
        serverEntry +
        '\n' +
        '    envVars:\n' +
        '      - key: NODE_VERSION\n' +
        '        value: ">=24.19.0 <25"\n'

      return {
        name: 'render',
        target: 'node',
        platform: 'render',
        runtime: 'node',
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        configFiles: ['render.yaml'],
        artifacts: [
          {
            kind: 'function',
            path: 'deploy/render/server',
            handlerSource: standaloneServerSource({ runtimePolicy: runtimeBuildPolicy(ctx) }),
          },
          { kind: 'static-site', path: 'deploy/render/public', optional: true },
          {
            kind: 'file',
            path: 'deploy/render/render.yaml',
            contents: renderBlueprint,
          },
          {
            kind: 'file',
            path: 'deploy/render/README.md',
            contents:
              '# Ruvyxa on Render\n\n' +
              'Render auto-detects this adapter through `RENDER=true`.\n' +
              'The generated server honors `PORT` and binds to `0.0.0.0`.\n\n' +
              '```bash\nnode ' +
              serverEntry +
              '\n```\n',
          },
          ...(options.projectConfig === false
            ? []
            : [
                {
                  kind: 'file',
                  path: 'render.yaml',
                  scope: 'project',
                  skipIfExists: true,
                  contents: renderBlueprint,
                } satisfies AdapterArtifact,
              ]),
        ],
      }
    },
  }
}

export default render
