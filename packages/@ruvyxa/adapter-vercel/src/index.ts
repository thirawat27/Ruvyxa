import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, staticAssetPattern, validateBuildContext } from '@ruvyxa/core'

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
  /**
   * Vercel region codes the serverless function runs in, closest first
   * (for example `['sin1']` for Singapore).
   *
   * Static pages are served from the edge everywhere, but an SSR page, an API
   * route, or an ISR revalidation runs in the function region — `iad1` (US
   * East) unless the account or this option says otherwise, which adds a
   * cross-continent round trip for users far from it. Left unset, Vercel's own
   * default applies.
   */
  regions?: string[]
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
// Imported, not read from disk: a platform that re-bundles the function only
// carries files it can resolve statically (see the netlify adapter, where a
// readFileSync of a sibling manifest.json crashed the deployed function).
import manifest from './manifest.mjs';
import { readFileSync, writeFileSync, mkdirSync, statSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const prerenderDir = path.join(import.meta.dirname, 'prerender');
// The function bundle directory is read-only at runtime; only the platform
// tmp directory accepts writes. ISR revalidations land there and are read
// back before the bundled deploy-time prerender output.
const isrCacheDir = path.join(os.tmpdir(), 'ruvyxa-isr-cache');

const readEntry = (htmlPath, revalidate) => {
  const html = readFileSync(htmlPath, 'utf8');
  const stale = Date.now() - statSync(htmlPath).mtimeMs >= revalidate * 1000;
  return { html, stale };
};

const handler = createHandler({
  routes: manifest.routes,
  importPage: loadRouteModule,
  importApi: loadRouteModule,
  readPrerendered: (pathname, revalidate = 60) => {
    // prerenderRelativePath rejects any request path that cannot be mapped to a
    // location inside the cache directories, so reads can never escape them.
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return null;
    try {
      return readEntry(path.join(isrCacheDir, relative), revalidate);
    } catch {
      // fall through to the bundled prerender output
    }
    try {
      return readEntry(path.join(prerenderDir, relative), revalidate);
    } catch {
      return null;
    }
  },
  writePrerendered: (pathname, html, revalidate) => {
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return;
    const htmlPath = path.join(isrCacheDir, relative);
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

  if (
    options.regions !== undefined &&
    (!Array.isArray(options.regions) ||
      options.regions.length === 0 ||
      options.regions.some((region) => typeof region !== 'string' || region.trim() === ''))
  ) {
    throw new Error(
      `[RUV2001] vercelAdapter: "regions" must be a non-empty array of region codes, such as ["sin1"]`,
    )
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
      const STATIC_ASSET_PATTERN = staticAssetPattern()

      // Build Output API v3 config with dynamic routing
      const buildOutputConfig = JSON.stringify(
        {
          version: 3,
          routes: [
            {
              // Hashed client bundles are served under /__ruvyxa/client/
              src: '^/__ruvyxa/client/(.*)$',
              headers: { 'cache-control': 'public, max-age=31536000, immutable' },
              continue: true,
            },
            {
              // Public assets are not content-hashed, so they revalidate rather
              // than being cached forever. Without this Vercel serves them with
              // `max-age=0, must-revalidate` and every navigation re-fetches
              // each image and font. Matches the header the Rust server sends
              // for the same files (`serve_public_file`).
              src: STATIC_ASSET_PATTERN,
              headers: { 'cache-control': 'public, max-age=3600, must-revalidate' },
              continue: true,
            },
            // Static assets served from filesystem
            { handle: 'filesystem' },
            {
              // Reached only when the filesystem missed. An asset-shaped path
              // with no file behind it is a 404, not a page: otherwise a bare
              // dynamic route such as `/[lang]` captures `/logo.png` and the
              // function answers 200 with an HTML body, which browsers show as
              // a broken image. It also kept every favicon miss paying for a
              // function invocation in the function region.
              src: STATIC_ASSET_PATTERN,
              status: 404,
            },
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
          ...(options.regions === undefined ? {} : { regions: options.regions }),
        },
        null,
        2,
      )

      const projectArtifacts: AdapterOutput['artifacts'] =
        options.projectOutput === false
          ? []
          : [
              // `optional`: an API-only or all-SSR app has no prerendered
              // pages; the function still serves every route (see the node
              // adapter, which set this precedent).
              {
                kind: 'static-site',
                path: '.vercel/output/static',
                scope: 'project',
                optional: true,
                // `handle: filesystem` runs before the function, so a
                // published ISR/PPR page would be served forever from its
                // build-time snapshot and never revalidate.
                excludeStrategies: ['isr', 'ppr'],
              },
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
          // Static assets. `optional`: an API-only or all-SSR app has no
          // prerendered pages; the serverless function still serves every
          // route, so the missing prerender directory must not fail the build.
          {
            kind: 'static-site',
            path: 'deploy/vercel/.vercel/output/static',
            optional: true,
            excludeStrategies: ['isr', 'ppr'],
          },
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
