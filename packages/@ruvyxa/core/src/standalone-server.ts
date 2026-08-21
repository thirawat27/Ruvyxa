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
import { createHandler, parseByteRange, prerenderRelativePath } from './serverless-handler.mjs';
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

  // Ranges, decided by the same table the Rust server answers. This host and
  // \`ruvyxa start\` serve the same public/ directory, so a video that scrubs
  // under one has to scrub under the other.
  const range = parseByteRange(req.headers.range ?? '', hit.size);
  if (range.kind === 'unsatisfiable') {
    res.statusCode = 416;
    res.setHeader('accept-ranges', 'bytes');
    res.setHeader('content-range', 'bytes */' + hit.size);
    res.end();
    return;
  }
  const partial = range.kind === 'partial' ? range : null;

  res.statusCode = partial ? 206 : 200;
  res.setHeader('content-type', contentType);
  res.setHeader('accept-ranges', 'bytes');
  res.setHeader('content-length', partial ? partial.end - partial.start + 1 : hit.size);
  if (partial) {
    res.setHeader('content-range', 'bytes ' + partial.start + '-' + partial.end + '/' + hit.size);
  }
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
  const file = partial
    ? createReadStream(hit.file, { start: partial.start, end: partial.end })
    : createReadStream(hit.file);
  // The response headers are already out, so there is no status left to send:
  // a read that fails here (the file was replaced mid-deploy, a disk error)
  // can only be turned into a broken connection, which is what a truncated
  // body would look like anyway. Without this listener the 'error' event is
  // unhandled and takes the whole process down.
  file.on('error', (error) => {
    console.error('[ruvyxa] static read failed:', error);
    res.destroy(error);
  });
  // A client that disconnects mid-download leaves the read stream open;
  // closing it stops the file descriptor and the buffering behind it.
  res.on('close', () => file.destroy());
  file.pipe(res);
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
    const body = Readable.fromWeb(response.body);
    // Status and headers are already committed by the time a stream fails, so
    // the only honest signal left is to drop the connection. Unhandled, this
    // event would terminate the process and take every concurrent request with
    // it — one aborted download must not be able to do that.
    body.on('error', (error) => {
      console.error('[ruvyxa] response stream failed:', error);
      res.destroy(error);
    });
    // Stop producing for a client that has gone away: without this an aborted
    // navigation keeps the render running to completion for nobody.
    res.on('close', () => body.destroy());
    body.pipe(res);
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

function positiveMs(name, fallback) {
  const raw = Number(process.env[name]);
  return Number.isFinite(raw) && raw > 0 ? raw : fallback;
}

// Keep-alive has to outlive the proxy in front of it. Node closes an idle
// connection after 5 seconds by default, while AWS ALB idles at 60 and most
// other managed load balancers sit at or above that. When the proxy believes a
// pooled socket is still good and Node has already begun closing it, the
// request that lands on it fails as a 502 — intermittently, under load, and
// only in production. Staying above the proxy's idle window makes the proxy,
// not the origin, the side that retires a connection.
server.keepAliveTimeout = positiveMs('RUVYXA_KEEP_ALIVE_TIMEOUT', 65_000);
// Must exceed keepAliveTimeout, or Node can time out the headers of a request
// arriving on a connection it was still willing to keep.
server.headersTimeout = positiveMs('RUVYXA_HEADERS_TIMEOUT', server.keepAliveTimeout + 5_000);

// How long in-flight work may finish after a shutdown signal. Platforms send
// SIGTERM and then SIGKILL after their own grace period (commonly 30s), so
// this stays under the usual floor.
const SHUTDOWN_GRACE_MS = positiveMs('RUVYXA_SHUTDOWN_GRACE', 25_000);

let shuttingDown = false;

function shutdown(reason, exitCode) {
  if (shuttingDown) return;
  shuttingDown = true;
  console.log(\`[ruvyxa] \${reason}: draining connections\`);

  // Stop accepting new connections and wait for in-flight responses. Without
  // this a deploy kills the process outright and every request being served at
  // that moment fails in the user's browser.
  server.close(() => {
    clearTimeout(forceExit);
    console.log('[ruvyxa] shutdown complete');
    process.exit(exitCode);
  });

  // Idle keep-alive sockets hold the close callback for as long as
  // keepAliveTimeout, which would make every deploy wait a full minute for
  // connections carrying nothing. Requests in progress are unaffected.
  if (typeof server.closeIdleConnections === 'function') server.closeIdleConnections();

  // A request that never finishes must not outlive the platform's own grace
  // period, or the process is SIGKILLed and the drain was pointless.
  const forceExit = setTimeout(() => {
    console.error('[ruvyxa] drain timed out; exiting with requests still open');
    process.exit(exitCode);
  }, SHUTDOWN_GRACE_MS);
  forceExit.unref();
}

for (const signal of ['SIGTERM', 'SIGINT']) {
  process.on(signal, () => shutdown(\`received \${signal}\`, 0));
}

// One route that rejects outside the request's own try/catch would otherwise
// terminate the process — Node's default for an unhandled rejection — and take
// every other in-flight request down with it. A single bad request must not be
// able to do that, so this is reported and the server keeps serving.
process.on('unhandledRejection', (reason) => {
  console.error('[ruvyxa] unhandled promise rejection:', reason);
});

// An uncaught exception is different: the process state after it is undefined,
// so continuing to serve from it is not trustworthy. Drain what is in flight,
// then leave with a non-zero code so the supervisor restarts a clean process.
process.on('uncaughtException', (error) => {
  console.error('[ruvyxa] uncaught exception:', error);
  shutdown('uncaught exception', 1);
});

server.listen(port, host, () => {
  console.log(\`[ruvyxa] standalone server listening on http://\${host === '0.0.0.0' ? 'localhost' : host}:\${port}\`);
});
`
}
