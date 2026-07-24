import {
  CLIENT_BUNDLE_PREFIX,
  IMMUTABLE_CACHE_CONTROL,
  PUBLIC_ASSET_CACHE_CONTROL,
  STATIC_ASSET_EXTENSIONS,
} from './utils.js'

/**
 * Source for the self-contained HTTP server that the node and bun adapters
 * emit.
 *
 * Wraps the generic Ruvyxa serverless handler in a plain `node:http` server
 * that also serves the publish directory, so the emitted `deploy/<runtime>/`
 * tree runs on any Node-compatible host (Docker, PM2, systemd, any PaaS, Bun)
 * without the ruvyxa CLI or its native binary installed at runtime.
 *
 * Shared rather than duplicated: this file decides request ordering, static
 * fallbacks, and cache headers, and those decisions have to stay identical
 * across every runtime that serves a Ruvyxa build.
 */
export function standaloneServerSource(): string {
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
