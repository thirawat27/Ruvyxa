import type { Adapter, AdapterArtifact, AdapterOutput, BuildContext } from '@ruvyxa/core'
import {
  CLIENT_BUNDLE_PREFIX,
  clientBuildOutput,
  IMMUTABLE_CACHE_CONTROL,
  projectRelativeOutDir,
  PUBLIC_ASSET_CACHE_CONTROL,
  publicAssetGlobs,
  runtimeBuildPolicy,
  validateBuildContext,
} from '@ruvyxa/core'

/** Node.js runtimes Cloud Functions (2nd gen) accepts. */
export type FirebaseRuntime = 'nodejs20' | 'nodejs22' | 'nodejs24'

/** Options for Firebase Hosting and Cloud Functions deployments. */
export interface FirebaseAdapterOptions {
  /** Cloud Functions export and Hosting rewrite ID. @default "ruvyxaServer" */
  functionName?: string
  /** Cloud Functions region colocated with Firebase Hosting. @default "us-central1" */
  region?: string
  /**
   * Cloud Functions runtime. Node 24 is generally available on the
   * second-generation functions this adapter emits; the older runtimes are
   * here for a project pinned to one.
   * @default "nodejs24"
   */
  runtime?: FirebaseRuntime
  /**
   * Emit a project-root `firebase.json`. Existing configuration is never overwritten.
   * @default true
   */
  projectConfig?: boolean
}

/** Create a Firebase Hosting adapter backed by a second-generation HTTPS function. */
export function firebase(options: FirebaseAdapterOptions = {}): Adapter {
  const functionName = options.functionName ?? 'ruvyxaServer'
  const region = options.region ?? 'us-central1'
  const runtime = options.runtime ?? 'nodejs24'
  // The function package's `engines.node` has to name the same major as
  // firebase.json, or the Firebase CLI rejects the deploy over the mismatch.
  const nodeMajor = runtime.replace('nodejs', '')

  if (!/^[A-Za-z_$][A-Za-z0-9_$]{0,62}$/.test(functionName)) {
    throw new Error(
      '[RUV2001] firebaseAdapter: "functionName" must be a valid JavaScript identifier',
    )
  }
  if (!['nodejs20', 'nodejs22', 'nodejs24'].includes(runtime)) {
    throw new Error('[RUV2001] firebaseAdapter: "runtime" must be nodejs20, nodejs22, or nodejs24')
  }
  if (!/^[a-z]+-[a-z]+\d+$/.test(region)) {
    throw new Error(
      '[RUV2001] firebaseAdapter: "region" must be a Google Cloud region such as asia-east1',
    )
  }

  return {
    name: 'firebase',
    target: 'serverless',
    supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'firebaseAdapter')

      // `firebase deploy` resolves `functions.source` and `hosting.public`
      // relative to the directory holding firebase.json. The project-root copy
      // therefore has to name this build's `outDir` — hard-coding `.ruvyxa`
      // pointed a project that configures `outDir` at a directory that does not
      // exist — while the copy inside the deploy directory names its own
      // siblings, so `firebase deploy` also works from there.
      const relativeOutDir = projectRelativeOutDir(ctx)
      const firebaseConfigFor = (functionsSource: string, hostingPublic: string) =>
        JSON.stringify(
          {
            functions: [
              {
                source: functionsSource,
                codebase: 'ruvyxa',
                runtime,
              },
            ],
            hosting: {
              public: hostingPublic,
              ignore: ['firebase.json', '**/.*', '**/node_modules/**'],
              headers: [
                {
                  source: `${CLIENT_BUNDLE_PREFIX}**`,
                  headers: [{ key: 'Cache-Control', value: IMMUTABLE_CACHE_CONTROL }],
                },
                ...publicAssetGlobs().map((glob) => ({
                  source: `**${glob}`,
                  headers: [{ key: 'Cache-Control', value: PUBLIC_ASSET_CACHE_CONTROL }],
                })),
              ],
              rewrites: [
                {
                  source: '**',
                  function: {
                    functionId: functionName,
                    region,
                    pinTag: true,
                  },
                },
              ],
            },
          },
          null,
          2,
        )

      const projectFirebaseConfig = firebaseConfigFor(
        `${relativeOutDir}/deploy/firebase/functions`,
        `${relativeOutDir}/deploy/firebase/public`,
      )
      const deployFirebaseConfig = firebaseConfigFor('functions', 'public')

      const functionPackage = JSON.stringify(
        {
          name: 'ruvyxa-firebase-functions',
          private: true,
          type: 'module',
          main: 'index.mjs',
          engines: { node: nodeMajor },
          dependencies: { 'firebase-functions': '^7.3.0' },
        },
        null,
        2,
      )

      return {
        name: 'firebase',
        target: 'serverless',
        platform: 'firebase',
        runtime: 'node',
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        functionsDir: `${ctx.outDir}/deploy/firebase/functions`,
        configFiles: ['firebase.json'],
        artifacts: [
          {
            kind: 'static-site',
            path: 'deploy/firebase/public',
            optional: true,
            excludeStrategies: ['isr', 'ppr'],
          },
          {
            kind: 'function',
            path: 'deploy/firebase/functions',
            handlerSource: firebaseHandlerSource(functionName, region, runtimeBuildPolicy(ctx)),
          },
          {
            kind: 'file',
            path: 'deploy/firebase/functions/package.json',
            contents: functionPackage + '\n',
          },
          {
            kind: 'file',
            path: 'deploy/firebase/firebase.json',
            contents: deployFirebaseConfig + '\n',
          },
          {
            kind: 'file',
            path: 'deploy/firebase/README.md',
            contents:
              '# Ruvyxa on Firebase Hosting\n\n' +
              'Select a Firebase project, build, then deploy Hosting and Functions together:\n\n' +
              '```bash\nruvyxa build --adapter firebase\nfirebase deploy --only hosting,functions\n```\n',
          },
          ...(options.projectConfig === false
            ? []
            : [
                {
                  kind: 'file',
                  path: 'firebase.json',
                  scope: 'project',
                  skipIfExists: true,
                  contents: projectFirebaseConfig + '\n',
                } satisfies AdapterArtifact,
              ]),
        ],
      }
    },
  }
}

/** Firebase Functions v2 wrapper around the shared Ruvyxa serverless handler. */
function firebaseHandlerSource(
  functionName: string,
  region: string,
  runtimePolicy: unknown,
): string {
  return `import { onRequest } from 'firebase-functions/v2/https';
import { createHandler, prerenderRelativePath } from './serverless-handler.mjs';
import { applyPluginHttp, loadActionModule, loadRouteModule } from './route-modules.mjs';
import manifest from './manifest.mjs';
import { readFileSync, writeFileSync, mkdirSync, statSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';

const runtimePolicy = ${JSON.stringify(runtimePolicy ?? {})};

const prerenderDir = path.join(import.meta.dirname, 'prerender');
const isrCacheDir = path.join(os.tmpdir(), 'ruvyxa-isr-cache');

const readEntry = (htmlPath, revalidate) => {
  const html = readFileSync(htmlPath, 'utf8');
  const stale = Date.now() - statSync(htmlPath).mtimeMs >= revalidate * 1000;
  return { html, stale };
};

const handler = createHandler({
  routes: manifest.routes,
  middleware: runtimePolicy.middleware,
  i18n: manifest.i18n,
  importPage: loadRouteModule,
  importApi: loadRouteModule,
  importAction: loadActionModule,
  pluginHttp: applyPluginHttp,
  security: runtimePolicy.security,
  readPrerendered: (pathname, revalidate = 60) => {
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return null;
    for (const directory of [isrCacheDir, prerenderDir]) {
      try {
        return readEntry(path.join(directory, relative), revalidate);
      } catch {
        // try the deploy-time prerender output after the runtime cache
      }
    }
    return null;
  },
  writePrerendered: (pathname, html) => {
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return;
    const htmlPath = path.join(isrCacheDir, relative);
    mkdirSync(path.dirname(htmlPath), { recursive: true });
    writeFileSync(htmlPath, html, 'utf8');
  },
  supportedStrategies: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
});

async function requestBody(req) {
  if (req.rawBody) return req.rawBody;
  if (req.body === undefined || req.body === null) return undefined;
  if (typeof req.body === 'string' || ArrayBuffer.isView(req.body)) return req.body;
  const contentType = String(req.headers['content-type'] ?? '');
  if (contentType.includes('application/x-www-form-urlencoded')) {
    return new URLSearchParams(req.body).toString();
  }
  return JSON.stringify(req.body);
}

export const ${functionName} = onRequest(
  { region: ${JSON.stringify(region)}, timeoutSeconds: 60 },
  async (req, res) => {
    const url = new URL(req.originalUrl ?? req.url, \`https://\${req.headers.host || 'localhost'}\`);
    const headers = new Headers();
    for (const [key, value] of Object.entries(req.headers)) {
      if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
    }
    const requestInit = { method: req.method, headers };
    if (req.method !== 'GET' && req.method !== 'HEAD') requestInit.body = await requestBody(req);
    const response = await handler(new Request(url.toString(), requestInit));
    res.status(response.status);
    for (const [key, value] of response.headers.entries()) {
      if (key !== 'set-cookie') res.setHeader(key, value);
    }
    const cookies = response.headers.getSetCookie?.() ?? [];
    if (cookies.length > 0) res.setHeader('set-cookie', cookies);
    if (!response.body) {
      res.end();
      return;
    }
    // Piped rather than collected into one buffer, matching the standalone
    // server and the Vercel function: a second-generation function runs on
    // Cloud Run, which forwards bytes as they are written, so buffering only
    // added the whole response to this instance's memory and pushed the first
    // byte back to the last.
    await pipeline(Readable.fromWeb(response.body), res);
  },
);
`
}

export default firebase
