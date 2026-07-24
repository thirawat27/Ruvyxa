import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import {
  CLIENT_BUNDLE_PREFIX,
  clientBuildOutput,
  IMMUTABLE_CACHE_CONTROL,
  PUBLIC_ASSET_CACHE_CONTROL,
  STATIC_ASSET_EXTENSIONS,
  validateBuildContext,
} from '@ruvyxa/core'

/**
 * Options for the Node.js adapter.
 */
export interface NodeAdapterOptions {
  /** Custom entry point path. Defaults to `${outDir}/server/app`. */
  entry?: string
}

/**
 * Standalone Node.js HTTP server source code.
 *
 * Wraps the generic Ruvyxa serverless handler into a plain `node:http`
 * server that also serves the static publish directory. The emitted
 * `deploy/node/` directory is self-contained: it runs with
 * `node server/index.mjs` on any Node.js host (Docker, PM2, systemd, any
 * PaaS) without the ruvyxa CLI or native binary installed at runtime.
 */
function nodeServerSource(): string {
  return `import { createServer } from 'node:http';
import { createHandler, prerenderRelativePath } from './serverless-handler.mjs';
import { loadRouteModule } from './route-modules.mjs';
// Imported so the directory stays deployable through any bundler that a host
// puts in front of it, matching the serverless adapters.
import manifest from './manifest.mjs';
import { createReadStream, readFileSync, writeFileSync, mkdirSync, statSync } from 'node:fs';
import path from 'node:path';

const here = import.meta.dirname;
const prerenderDir = path.join(here, 'prerender');
const publicDir = path.resolve(here, '..', 'public');

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

const MIME_TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json',
  '.map': 'application/json',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.avif': 'image/avif',
  '.gif': 'image/gif',
  '.ico': 'image/x-icon',
  '.txt': 'text/plain; charset=utf-8',
  '.xml': 'application/xml',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.wasm': 'application/wasm',
};

// Resolve a request path to a file inside publicDir, or null. Containment is
// enforced by resolving and prefix-checking before touching the file system.
function resolveStaticFile(pathname) {
  let decoded;
  try {
    decoded = decodeURIComponent(pathname);
  } catch {
    return null;
  }
  if (decoded.includes('\\0')) return null;
  const resolved = path.resolve(publicDir, decoded.replace(/^\\/+/, ''));
  if (resolved !== publicDir && !resolved.startsWith(publicDir + path.sep)) return null;
  const candidates = decoded.endsWith('/')
    ? [path.join(resolved, 'index.html')]
    : [resolved, path.join(resolved, 'index.html'), resolved + '.html'];
  // Mirror the Rust server's resolve_public_asset: a PNG/JPEG URL still
  // resolves when the build published only the WebP output
  // (image.keepOriginal: false), so the same markup works under \`ruvyxa start\`
  // and under this standalone server.
  if (/\\.(?:png|jpe?g)$/i.test(resolved)) {
    candidates.push(resolved.replace(/\\.(?:png|jpe?g)$/i, '.webp'));
  }
  for (const candidate of candidates) {
    try {
      const stats = statSync(candidate);
      if (stats.isFile()) return { file: candidate, size: stats.size };
    } catch {
      // try the next candidate
    }
  }
  return null;
}

const ASSET_EXTENSIONS = new Set(${JSON.stringify(STATIC_ASSET_EXTENSIONS)});

// True when the last path segment names a static asset file. Matches
// isStaticAssetPath in serverless-handler.mjs.
function isAssetPath(pathname) {
  const segment = pathname.slice(pathname.lastIndexOf('/') + 1);
  const dot = segment.lastIndexOf('.');
  if (dot <= 0 || dot === segment.length - 1) return false;
  return ASSET_EXTENSIONS.has(segment.slice(dot + 1).toLowerCase());
}

function sendStatic(req, res, hit, pathname) {
  const contentType = MIME_TYPES[path.extname(hit.file).toLowerCase()] ?? 'application/octet-stream';
  res.statusCode = 200;
  res.setHeader('content-type', contentType);
  res.setHeader('content-length', hit.size);
  // Same cache policy the Rust server applies to the same files: hashed
  // bundles are immutable, everything else from public/ revalidates hourly
  // instead of on every navigation.
  if (pathname.startsWith(${JSON.stringify(CLIENT_BUNDLE_PREFIX)})) {
    res.setHeader('cache-control', ${JSON.stringify(IMMUTABLE_CACHE_CONTROL)});
  } else {
    res.setHeader('cache-control', ${JSON.stringify(PUBLIC_ASSET_CACHE_CONTROL)});
  }
  if (req.method === 'HEAD') {
    res.end();
    return;
  }
  createReadStream(hit.file).pipe(res);
}

async function readRequestBody(req) {
  const chunks = [];
  for await (const chunk of req) {
    chunks.push(typeof chunk === 'string' ? Buffer.from(chunk) : chunk);
  }
  return Buffer.concat(chunks);
}

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url, \`http://\${req.headers.host || 'localhost'}\`);
    const isRead = req.method === 'GET' || req.method === 'HEAD';

    // Hashed client bundles and asset-shaped paths are served before routing,
    // the order the Rust server uses. Page-shaped paths go through the handler
    // first so ISR revalidation and dynamic routes keep working; unmatched
    // paths fall back to static files.
    if (isRead && (url.pathname.startsWith('/__ruvyxa/') || isAssetPath(url.pathname))) {
      const hit = resolveStaticFile(url.pathname);
      if (hit) {
        sendStatic(req, res, hit, url.pathname);
        return;
      }
    }

    const headers = new Headers();
    for (const [key, value] of Object.entries(req.headers)) {
      if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
    }
    const requestInit = { method: req.method, headers };
    if (!isRead) {
      requestInit.body = await readRequestBody(req);
    }
    const request = new Request(url.toString(), requestInit);
    const response = await handler(request);

    if (response.status === 404 && isRead) {
      const hit = resolveStaticFile(url.pathname);
      if (hit) {
        sendStatic(req, res, hit, url.pathname);
        return;
      }
    }

    res.statusCode = response.status;
    for (const [key, value] of response.headers.entries()) {
      if (key === 'set-cookie') continue;
      res.setHeader(key, value);
    }
    const setCookies = response.headers.getSetCookie?.() ?? [];
    if (setCookies.length > 0) res.setHeader('set-cookie', setCookies);
    if (req.method === 'HEAD') {
      res.end();
      return;
    }
    const body = Buffer.from(await response.arrayBuffer());
    res.end(body);
  } catch (error) {
    console.error('[ruvyxa] request failed:', error instanceof Error ? error.message : error);
    if (!res.headersSent) {
      res.statusCode = 500;
      res.setHeader('content-type', 'text/plain; charset=utf-8');
    }
    res.end('Internal Server Error');
  }
});

const port = Number(process.env.PORT || 3000);
const host = process.env.HOST || '0.0.0.0';
server.listen(port, host, () => {
  console.log(\`[ruvyxa] standalone server listening on http://\${host === '0.0.0.0' ? 'localhost' : host}:\${port}\`);
});
`
}

/**
 * Create a Node.js deployment adapter for Ruvyxa.
 *
 * Produces a self-contained standalone server in `deploy/node/`:
 * `server/index.mjs` (a plain `node:http` server around the generic
 * serverless handler) plus a `public/` directory with pre-rendered pages and
 * hashed client bundles. Runs on any Node.js hosting (Docker, PM2, systemd,
 * any PaaS) with `node server/index.mjs` — no ruvyxa CLI or native binary is
 * needed at runtime. Honors `PORT` and `HOST`.
 *
 * @example
 * ```ts
 * import { config } from "ruvyxa/config"
 * import { nodeAdapter } from "@ruvyxa/adapter-node"
 *
 * export default config({
 *   adapter: nodeAdapter()
 * })
 * ```
 */
export function nodeAdapter(options: NodeAdapterOptions = {}): Adapter {
  if (options.entry !== undefined && typeof options.entry !== 'string') {
    throw new Error(`[RUV2001] nodeAdapter: "entry" must be a string, got ${typeof options.entry}`)
  }

  if (options.entry !== undefined && options.entry.trim() === '') {
    throw new Error(`[RUV2001] nodeAdapter: "entry" must not be an empty string`)
  }

  return {
    name: 'node',
    target: 'node',
    supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'nodeAdapter')
      return {
        name: 'node',
        target: 'node',
        platform: 'node',
        entry: options.entry ?? `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        artifacts: [
          // Standalone server: compiled route registry + handler runtime
          {
            kind: 'function',
            path: 'deploy/node/server',
            handlerSource: nodeServerSource(),
          },
          // Static publish directory served by the standalone server. An
          // API-only app has no prerendered pages; the server still runs.
          { kind: 'static-site', path: 'deploy/node/public', optional: true },
          {
            kind: 'file',
            path: 'deploy/node/start.mjs',
            // npx resolves to npx.cmd on Windows, which spawn() refuses to
            // execute without a shell; keep the shell off elsewhere.
            contents: `import { spawn } from 'node:child_process'\n\nconst child = spawn('npx', ['--no-install', 'ruvyxa', 'start'], { cwd: process.cwd(), stdio: 'inherit', shell: process.platform === 'win32' })\nchild.on('exit', (code, signal) => process.exitCode = code ?? (signal ? 1 : 0))\n`,
          },
          {
            kind: 'file',
            path: 'deploy/node/README.md',
            contents:
              '# Ruvyxa Node deployment\n\n' +
              'Standalone (no ruvyxa runtime dependency):\n\n' +
              '```bash\nnode .ruvyxa/deploy/node/server/index.mjs\n```\n\n' +
              'Honors `PORT` (default 3000) and `HOST` (default 0.0.0.0). Copy the\n' +
              '`deploy/node/` directory anywhere — Docker, PM2, systemd, any PaaS —\n' +
              'and run the same command.\n\n' +
              'Alternative, using the installed ruvyxa CLI:\n\n' +
              '```bash\nnode .ruvyxa/deploy/node/start.mjs\n```\n',
          },
        ],
      }
    },
  }
}

export default nodeAdapter
