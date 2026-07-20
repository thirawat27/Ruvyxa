import type { Adapter, AdapterArtifact, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, validateBuildContext } from '@ruvyxa/core'

/**
 * Options for Vercel deployment.
 */
export interface VercelAdapterOptions {
  /** Custom functions output directory. Defaults to `${outDir}/functions`. */
  functionsDir?: string
  /**
   * Also emit the Build Output API directory at the project root
   * (`.vercel/output/`), which Vercel picks up automatically after
   * `ruvyxa build` runs — no dashboard output-directory configuration needed.
   * @default true
   */
  projectOutput?: boolean
  /**
   * Node.js runtime version for serverless functions.
   * @default 'nodejs20.x'
   */
  runtime?: string
  /**
   * Maximum execution duration in seconds for serverless functions.
   * @default 10
   */
  maxDuration?: number
}

/**
 * Vercel serverless function handler source code.
 *
 * Wraps the generic Ruvyxa serverless handler into a Vercel Build Output API
 * serverless function (Node.js runtime). Reads the route manifest and handles
 * SSR/API/ISR/PPR requests.
 */
function vercelHandlerSource(): string {
  return `import { createHandler } from './serverless-handler.mjs';
import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import path from 'node:path';

const manifestPath = path.join(import.meta.dirname, 'manifest.json');
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
const prerenderDir = path.join(import.meta.dirname, 'prerender');

const handler = createHandler({
  routes: manifest.routes,
  importPage: async (routeId) => {
    const route = manifest.routes.find(r => r.id === routeId);
    if (!route) throw new Error(\`Route \${routeId} not found in manifest\`);
    return import(\`./server/app/\${route.file}\`);
  },
  importApi: async (routeId) => {
    const route = manifest.routes.find(r => r.id === routeId);
    if (!route) throw new Error(\`Route \${routeId} not found in manifest\`);
    return import(\`./server/app/\${route.file}\`);
  },
  readPrerendered: (pathname) => {
    const htmlPath = pathname === '/'
      ? path.join(prerenderDir, 'index.html')
      : path.join(prerenderDir, pathname, 'index.html');
    try {
      return readFileSync(htmlPath, 'utf8');
    } catch {
      return null;
    }
  },
  writePrerendered: (pathname, html, revalidate) => {
    const htmlPath = pathname === '/'
      ? path.join(prerenderDir, 'index.html')
      : path.join(prerenderDir, pathname, 'index.html');
    try {
      mkdirSync(path.dirname(htmlPath), { recursive: true });
      writeFileSync(htmlPath, html, 'utf8');
    } catch {
      // ISR cache write failures are non-fatal
    }
  },
  supportedStrategies: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
});

export default async function(req, res) {
  const url = new URL(req.url, \`http://\${req.headers.host || 'localhost'}\`);
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) {
    if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
  }
  const requestInit = { method: req.method, headers };
  if (req.method !== 'GET' && req.method !== 'HEAD' && req.body) {
    requestInit.body = req.body;
  }
  const request = new Request(url.toString(), requestInit);
  const response = await handler(request);
  res.statusCode = response.status;
  for (const [key, value] of response.headers.entries()) {
    res.setHeader(key, value);
  }
  const body = await response.text();
  res.end(body);
}
`
}

/**
 * Create a Vercel deployment adapter for Ruvyxa.
 *
 * Produces serverless functions and static assets compatible with Vercel's
 * Build Output API v3. Supports SSR, API routes, ISR, PPR, SSG, and CSR.
 *
 * @example
 * ```ts
 * import { config } from "ruvyxa/config"
 * import { vercelAdapter } from "@ruvyxa/adapter-vercel"
 *
 * export default config({
 *   adapter: vercelAdapter()
 * })
 * ```
 */
export function vercelAdapter(options: VercelAdapterOptions = {}): Adapter {
  if (options.functionsDir !== undefined && typeof options.functionsDir !== 'string') {
    throw new Error(
      `[RUV2001] vercelAdapter: "functionsDir" must be a string, got ${typeof options.functionsDir}`,
    )
  }

  if (options.functionsDir !== undefined && options.functionsDir.trim() === '') {
    throw new Error(`[RUV2001] vercelAdapter: "functionsDir" must not be an empty string`)
  }

  return {
    name: 'vercel',
    target: 'serverless',
    supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'vercelAdapter')
      const functionsDir = options.functionsDir ?? `${ctx.outDir}/functions`
      const runtime = options.runtime ?? 'nodejs20.x'
      const maxDuration = options.maxDuration ?? 10

      // Build Output API v3 config with dynamic routing
      const buildOutputConfig = JSON.stringify(
        {
          version: 3,
          routes: [
            {
              src: '^/client/(.*)$',
              headers: { 'cache-control': 'public, max-age=31536000, immutable' },
              continue: true,
            },
            // Static assets served from filesystem
            { handle: 'filesystem' },
            // All unmatched requests go to the serverless function
            { src: '/(.*)', dest: '/__ruvyxa_handler' },
          ],
        },
        null,
        2,
      )

      // Vercel function configuration
      const vcConfig = JSON.stringify(
        {
          runtime,
          handler: 'index.mjs',
          maxDuration,
          launcherType: 'Nodejs',
        },
        null,
        2,
      )

      const projectArtifacts: AdapterOutput['artifacts'] =
        options.projectOutput === false
          ? []
          : [
              { kind: 'static-site', path: '.vercel/output/static', scope: 'project' },
              {
                kind: 'function',
                path: '.vercel/output/functions/__ruvyxa_handler.func',
                scope: 'project',
                handlerSource: vercelHandlerSource(),
              },
              {
                kind: 'file',
                path: '.vercel/output/functions/__ruvyxa_handler.func/.vc-config.json',
                scope: 'project',
                contents: vcConfig + '\n',
              },
              {
                kind: 'file',
                path: '.vercel/output/config.json',
                scope: 'project',
                contents: buildOutputConfig + '\n',
              },
            ]

      return {
        name: 'vercel',
        target: 'serverless',
        platform: 'vercel',
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        functionsDir,
        configFiles: ['vercel.json'],
        artifacts: [
          // Static assets
          { kind: 'static-site', path: 'deploy/vercel/.vercel/output/static' },
          // Serverless function bundle
          {
            kind: 'function',
            path: 'deploy/vercel/.vercel/output/functions/__ruvyxa_handler.func',
            handlerSource: vercelHandlerSource(),
          },
          // Function config
          {
            kind: 'file',
            path: 'deploy/vercel/.vercel/output/functions/__ruvyxa_handler.func/.vc-config.json',
            contents: vcConfig + '\n',
          },
          // Build Output API config
          {
            kind: 'file',
            path: 'deploy/vercel/.vercel/output/config.json',
            contents: buildOutputConfig + '\n',
          },
          ...projectArtifacts,
        ],
      }
    },
  }
}

export default vercelAdapter
