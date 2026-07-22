import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
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
  return `import { createHandler, prerenderRelativePath } from './serverless-handler.mjs';
import { loadRouteModule } from './route-modules.mjs';
import { readFileSync, writeFileSync, mkdirSync, statSync } from 'node:fs';
import path from 'node:path';

const manifestPath = path.join(import.meta.dirname, 'manifest.json');
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
const prerenderDir = path.join(import.meta.dirname, 'prerender');

const handler = createHandler({
  routes: manifest.routes,
  importPage: loadRouteModule,
  importApi: loadRouteModule,
  readPrerendered: (pathname, revalidate = 60) => {
    // prerenderRelativePath rejects any request path that cannot be mapped to a
    // location inside prerenderDir, so the cache read can never escape it.
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return null;
    try {
      const htmlPath = path.join(prerenderDir, relative);
      const html = readFileSync(htmlPath, 'utf8');
      const stale = Date.now() - statSync(htmlPath).mtimeMs >= revalidate * 1000;
      return { html, stale };
    } catch {
      return null;
    }
  },
  writePrerendered: (pathname, html, revalidate) => {
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return;
    const htmlPath = path.join(prerenderDir, relative);
    try {
      mkdirSync(path.dirname(htmlPath), { recursive: true });
      writeFileSync(htmlPath, html, 'utf8');
    } catch {
      // ISR cache write failures are non-fatal
    }
  },
  supportedStrategies: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
});

async function readRequestBody(req) {
  const parsed = req.body;
  if (parsed !== undefined && parsed !== null) {
    if (parsed instanceof ReadableStream) {
      return new Uint8Array(await new Response(parsed).arrayBuffer());
    }
    if (
      typeof parsed === 'string' ||
      parsed instanceof ArrayBuffer ||
      ArrayBuffer.isView(parsed) ||
      parsed instanceof Blob ||
      parsed instanceof FormData ||
      parsed instanceof URLSearchParams
    ) {
      return parsed;
    }
    const contentType = String(req.headers['content-type'] ?? '');
    if (contentType.includes('application/x-www-form-urlencoded')) {
      return new URLSearchParams(parsed).toString();
    }
    return JSON.stringify(parsed);
  }
  const chunks = [];
  for await (const chunk of req) {
    chunks.push(typeof chunk === 'string' ? Buffer.from(chunk) : chunk);
  }
  return Buffer.concat(chunks);
}

export default async function(req, res, context) {
  const url = new URL(req.url, \`http://\${req.headers.host || 'localhost'}\`);
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) {
    if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
  }
  const requestInit = { method: req.method, headers };
  if (req.method !== 'GET' && req.method !== 'HEAD') {
    try {
      requestInit.body = await readRequestBody(req);
    } catch {
      res.statusCode = 400;
      res.end('Bad Request');
      return;
    }
  }
  const request = new Request(url.toString(), requestInit);
  const response = await handler(request, context);
  res.statusCode = response.status;
  for (const [key, value] of response.headers.entries()) {
    if (key === 'set-cookie') continue;
    res.setHeader(key, value);
  }
  const setCookies = response.headers.getSetCookie?.() ?? [];
  if (setCookies.length > 0) res.setHeader('set-cookie', setCookies);
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
