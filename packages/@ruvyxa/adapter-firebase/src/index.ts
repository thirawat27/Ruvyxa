import type { Adapter, AdapterArtifact, AdapterOutput, BuildContext } from '@ruvyxa/core'
import {
  CLIENT_BUNDLE_PREFIX,
  clientBuildOutput,
  IMMUTABLE_CACHE_CONTROL,
  PUBLIC_ASSET_CACHE_CONTROL,
  publicAssetGlobs,
  runtimeBuildPolicy,
  validateBuildContext,
} from '@ruvyxa/core'

/** Options for Firebase Hosting and Cloud Functions deployments. */
export interface FirebaseAdapterOptions {
  /** Cloud Functions export and Hosting rewrite ID. @default "ruvyxaServer" */
  functionName?: string
  /** Cloud Functions region colocated with Firebase Hosting. @default "us-central1" */
  region?: string
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

  if (!/^[A-Za-z_$][A-Za-z0-9_$]{0,62}$/.test(functionName)) {
    throw new Error(
      '[RUV2001] firebaseAdapter: "functionName" must be a valid JavaScript identifier',
    )
  }
  if (!/^[a-z]+-[a-z]+[0-9]+$/.test(region)) {
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

      const firebaseConfig = JSON.stringify(
        {
          functions: [
            {
              source: '.ruvyxa/deploy/firebase/functions',
              codebase: 'ruvyxa',
              runtime: 'nodejs24',
            },
          ],
          hosting: {
            public: '.ruvyxa/deploy/firebase/public',
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

      const functionPackage = JSON.stringify(
        {
          name: 'ruvyxa-firebase-functions',
          private: true,
          type: 'module',
          main: 'index.mjs',
          engines: { node: '24' },
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
            contents: firebaseConfig + '\n',
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
                  contents: firebaseConfig + '\n',
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
    res.send(Buffer.from(await response.arrayBuffer()));
  },
);
`
}

export default firebase
