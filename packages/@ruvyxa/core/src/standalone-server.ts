import {
  CLIENT_BUNDLE_PREFIX,
  DEFAULT_SECURITY_HEADERS,
  FALLBACK_CONTENT_TYPE,
  IMMUTABLE_CACHE_CONTROL,
  PUBLIC_ASSET_CACHE_CONTROL,
  STATIC_ASSET_EXTENSIONS,
  STATIC_CONTENT_TYPES,
} from './utils.js'

/** Runtime-specific options for the generated standalone HTTP server. */
export interface StandaloneServerOptions {
  /**
   * Store ISR/PPR refreshes in the platform's writable temporary directory.
   * Use this for immutable server bundles such as AWS Amplify compute.
   * @default 'bundle'
   */
  isrCache?: 'bundle' | 'tmp'
  /** Validated build runtime policy embedded into the generated server. */
  runtimePolicy?: Readonly<Record<string, unknown>>
}

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
export function standaloneServerSource(options: StandaloneServerOptions = {}): string {
  const isrCacheDirectory =
    options.isrCache === 'tmp' ? "path.join(os.tmpdir(), 'ruvyxa-isr-cache')" : 'prerenderDir'

  return `import { createServer } from 'node:http';
import { createHandler, prerenderRelativePath } from './serverless-handler.mjs';
import { applyPluginHttp, loadActionModule, loadRouteModule } from './route-modules.mjs';
// Imported so the directory stays deployable through any bundler that a host
// puts in front of it, matching the serverless adapters.
import manifest from './manifest.mjs';
import { createReadStream, readFileSync, writeFileSync, mkdirSync, statSync } from 'node:fs';
import { Readable } from 'node:stream';
import os from 'node:os';
import path from 'node:path';

const runtimePolicy = ${JSON.stringify(options.runtimePolicy ?? {})};

const here = import.meta.dirname;
const prerenderDir = path.join(here, 'prerender');
const isrCacheDir = ${isrCacheDirectory};
const publicDir = path.resolve(here, '..', 'public');

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
    // prerenderRelativePath rejects any request path that cannot be mapped to a
    // location inside the selected cache root, so the cache read can never escape it.
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return null;
    const cacheDirectories =
      isrCacheDir === prerenderDir ? [prerenderDir] : [isrCacheDir, prerenderDir];
    for (const cacheDirectory of cacheDirectories) {
      try {
        const htmlPath = path.join(cacheDirectory, relative);
        const html = readFileSync(htmlPath, 'utf8');
        const stale = Date.now() - statSync(htmlPath).mtimeMs >= revalidate * 1000;
        return { html, stale };
      } catch {
        // try the deploy-time prerender output after the runtime cache
      }
    }
    return null;
  },
  writePrerendered: (pathname, html, revalidate) => {
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return;
    const htmlPath = path.join(isrCacheDir, relative);
    mkdirSync(path.dirname(htmlPath), { recursive: true });
    writeFileSync(htmlPath, html, 'utf8');
  },
  supportedStrategies: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
});

// Serialized from STATIC_CONTENT_TYPES so this server and the Rust one answer
// from the same table; see tests/fixtures/static-asset-conformance.json.
const MIME_TYPES = ${JSON.stringify(STATIC_CONTENT_TYPES)};

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
const DEFAULT_SECURITY_HEADERS = ${JSON.stringify(DEFAULT_SECURITY_HEADERS)};

// \`security.headers: false\` turns these off in \`ruvyxa start\`; this server has
// to agree, or the same project answers with different headers depending on
// which one is serving it.
const SECURITY_HEADERS_ENABLED = runtimePolicy.security?.headers !== false;

function applySecurityHeaders(res) {
  if (!SECURITY_HEADERS_ENABLED) return;
  for (const [name, value] of Object.entries(DEFAULT_SECURITY_HEADERS)) {
    if (!res.hasHeader(name)) res.setHeader(name, value);
  }
}

// Matches \`security.apiLimit\`. The body is buffered here before the handler
// sees it, so the handler's own cap would arrive too late to prevent the
// allocation — this is where the limit has to be enforced for this server.
const REQUEST_BODY_LIMIT = Number.isInteger(runtimePolicy.security?.apiLimit)
  && runtimePolicy.security.apiLimit > 0
  ? runtimePolicy.security.apiLimit
  : 10 * 1024 * 1024;

// True when the last path segment names a static asset file. Matches
// isStaticAssetPath in serverless-handler.mjs.
function isAssetPath(pathname) {
  const segment = pathname.slice(pathname.lastIndexOf('/') + 1);
  const dot = segment.lastIndexOf('.');
  if (dot <= 0 || dot === segment.length - 1) return false;
  return ASSET_EXTENSIONS.has(segment.slice(dot + 1).toLowerCase());
}

function sendStatic(req, res, hit, pathname) {
  // Keyed without the leading dot, and lowercased because a file system hands
  // back \`hero.PNG\` exactly as it was written.
  const contentType =
    MIME_TYPES[path.extname(hit.file).slice(1).toLowerCase()] ??
    ${JSON.stringify(FALLBACK_CONTENT_TYPE)};
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

class RequestBodyTooLarge extends Error {}

async function readRequestBody(req) {
  const chunks = [];
  let total = 0;
  for await (const chunk of req) {
    const bytes = typeof chunk === 'string' ? Buffer.from(chunk) : chunk;
    total += bytes.length;
    // Stop reading rather than finish buffering and reject afterwards: the
    // point of the limit is to bound what one request can allocate.
    if (total > REQUEST_BODY_LIMIT) throw new RequestBodyTooLarge();
    chunks.push(bytes);
  }
  return Buffer.concat(chunks);
}

const server = createServer(async (req, res) => {
  applySecurityHeaders(res);
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
    if (response.body === null) {
      res.end();
      return;
    }
    // Preserve streaming responses instead of buffering the complete body.
    // This lowers peak memory and lets the first chunk reach the client while
    // an SSR or API stream is still being produced.
    Readable.fromWeb(response.body).pipe(res);
  } catch (error) {
    if (error instanceof RequestBodyTooLarge) {
      if (!res.headersSent) {
        res.statusCode = 413;
        res.setHeader('content-type', 'text/plain; charset=utf-8');
      }
      res.end('Request body is too large');
      return;
    }
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
