import { readFileSync } from 'node:fs'

import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import {
  CLIENT_BUNDLE_PREFIX,
  clientBuildOutput,
  DEFAULT_SECURITY_HEADERS,
  IMMUTABLE_CACHE_CONTROL,
  nonPublishableStrategies,
  PUBLIC_ASSET_CACHE_CONTROL,
  runtimeBuildPolicy,
  standaloneServerSource,
  validateBuildContext,
} from '@ruvyxa/core'

type AmplifyRuntime = 'nodejs20.x' | 'nodejs22.x' | 'nodejs24.x'

/** Options for AWS Amplify Hosting deployments. */
export interface AwsAdapterOptions {
  /** Amplify compute runtime. @default "nodejs24.x" */
  runtime?: AmplifyRuntime
  /**
   * Emit the project-root `.amplify-hosting/` deployment bundle Amplify discovers.
   * @default true
   */
  projectOutput?: boolean
}

/** Create an AWS Amplify Hosting static-plus-compute adapter for Ruvyxa. */
export function aws(options: AwsAdapterOptions = {}): Adapter {
  const runtime = options.runtime ?? 'nodejs24.x'
  const frameworkVersion = packageVersion()

  return {
    name: 'aws',
    target: 'serverless',
    supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'awsAdapter')

      const manifest = JSON.stringify(
        {
          version: 1,
          routes: [
            {
              path: `${CLIENT_BUNDLE_PREFIX}*`,
              target: { kind: 'Static', cacheControl: IMMUTABLE_CACHE_CONTROL },
            },
            {
              path: '/*.*',
              target: { kind: 'Static', cacheControl: PUBLIC_ASSET_CACHE_CONTROL },
              fallback: { kind: 'Compute', src: 'default' },
            },
            {
              path: '/*',
              target: { kind: 'Compute', src: 'default' },
            },
          ],
          computeResources: [{ name: 'default', runtime, entrypoint: 'server.js' }],
          framework: { name: 'ruvyxa', version: frameworkVersion },
        },
        null,
        2,
      )
      // Amplify Hosting's custom-header file.
      //
      // The deploy manifest's route targets carry `cacheControl` and nothing
      // else, so it has no place to put the security defaults — and a `Static`
      // target is answered by the CDN, which never invokes the compute
      // resource where the standalone server sets them. That left a deployed
      // pre-rendered page without a single one of them while the identical page
      // from `ruvyxa start` carried all seven. `customHttp.yml` is the
      // mechanism Amplify documents for this, read from the app root.
      const customHttp =
        'customHeaders:\n' +
        `  - pattern: '**'\n` +
        '    headers:\n' +
        Object.entries(DEFAULT_SECURITY_HEADERS)
          .map(([key, value]) => `      - key: '${key}'\n        value: '${value}'\n`)
          .join('')

      const serverSource = standaloneServerSource({
        isrCache: 'tmp',
        runtimePolicy: runtimeBuildPolicy(ctx),
      })
      const deployRoot = 'deploy/aws/.amplify-hosting'

      const projectArtifacts: AdapterOutput['artifacts'] =
        options.projectOutput === false
          ? []
          : [
              {
                kind: 'static-site',
                path: '.amplify-hosting/static',
                scope: 'project',
                optional: true,
                excludeStrategies: nonPublishableStrategies(),
              },
              {
                kind: 'function',
                path: '.amplify-hosting/compute/default',
                scope: 'project',
                handlerSource: serverSource,
              },
              {
                kind: 'file',
                path: '.amplify-hosting/compute/default/server.js',
                scope: 'project',
                contents: "import './index.mjs'\n",
              },
              {
                kind: 'file',
                path: '.amplify-hosting/deploy-manifest.json',
                scope: 'project',
                contents: manifest + '\n',
              },
              {
                kind: 'file',
                path: 'customHttp.yml',
                scope: 'project',
                // Amplify reads one `customHttp.yml` per app and a project may
                // already have written its own rules into it. Overwriting that
                // every build would trade one silent header loss for another.
                skipIfExists: true,
                contents: customHttp,
              },
            ]

      return {
        name: 'aws',
        target: 'serverless',
        platform: 'aws',
        runtime: 'node',
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        configFiles: ['.amplify-hosting/deploy-manifest.json'],
        artifacts: [
          {
            kind: 'static-site',
            path: `${deployRoot}/static`,
            optional: true,
            excludeStrategies: nonPublishableStrategies(),
          },
          {
            kind: 'function',
            path: `${deployRoot}/compute/default`,
            handlerSource: serverSource,
          },
          {
            kind: 'file',
            path: `${deployRoot}/compute/default/server.js`,
            contents: "import './index.mjs'\n",
          },
          {
            kind: 'file',
            path: `${deployRoot}/deploy-manifest.json`,
            contents: manifest + '\n',
          },
          {
            kind: 'file',
            path: 'deploy/aws/customHttp.yml',
            contents: customHttp,
          },
          {
            kind: 'file',
            path: 'deploy/aws/README.md',
            contents:
              '# Ruvyxa on AWS Amplify Hosting\n\n' +
              'Amplify auto-detects this adapter through `AWS_APP_ID` and deploys\n' +
              'the generated `.amplify-hosting/` static and compute primitives.\n',
          },
          ...projectArtifacts,
        ],
      }
    },
  }
}

/** Read the adapter package version so Amplify receives valid framework semver metadata. */
function packageVersion(): string {
  const metadata = JSON.parse(
    readFileSync(new URL('../package.json', import.meta.url), 'utf8'),
  ) as {
    version?: unknown
  }
  if (typeof metadata.version !== 'string' || !/^\d+\.\d+\.\d+/.test(metadata.version)) {
    throw new Error('[RUV2001] awsAdapter: package version must be valid semantic version metadata')
  }
  return metadata.version
}

export default aws
