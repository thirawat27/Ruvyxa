import {
  CLIENT_BUNDLE_PREFIX,
  DEFAULT_SECURITY_HEADERS,
  FALLBACK_CONTENT_TYPE,
  IMMUTABLE_CACHE_CONTROL,
  PUBLIC_ASSET_CACHE_CONTROL,
  STATIC_ASSET_EXTENSIONS,
  STATIC_CONTENT_TYPES,
} from './utils.js'

/**
 * A runtime the standalone server can be emitted for.
 *
 * The three differ only in how a request reaches the program and how a file
 * becomes a response body. Everything between — routing order, cache headers,
 * ISR, security headers, byte ranges — is one shared program text, so the
 * question "does a Bun deployment behave like a Node one" is answered by
 * construction rather than by two implementations that agree today.
 */
export type StandaloneServerRuntime = 'node' | 'bun' | 'deno'

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
  /**
   * Which runtime will execute the emitted program.
   * @default 'node'
   */
  runtime?: StandaloneServerRuntime
}

/**
 * Source for the self-contained HTTP server that the node, bun, deno, aws,
 * railway, and render adapters emit.
 *
 * Wraps the generic Ruvyxa serverless handler in the host's own HTTP server and
 * also serves the publish directory, so the emitted `deploy/<runtime>/` tree
 * runs on any supported host (Docker, PM2, systemd, any PaaS, Bun, Deno)
 * without the ruvyxa CLI or its native binary installed at runtime.
 *
 * Shared rather than duplicated: this file decides request ordering, static
 * fallbacks, and cache headers, and those decisions have to stay identical
 * across every runtime that serves a Ruvyxa build.
 *
 * Bun and Deno both implement `node:http`, and both were served through it
 * until this became runtime-aware. It worked, and it made every request pay for
 * a `Request` being taken apart into a Node request and a `Response` being
 * reassembled from a Node one — on two runtimes whose native server *is* the
 * `Request` → `Response` function the handler already is. They call the handler
 * directly now.
 */
export function standaloneServerSource(options: StandaloneServerOptions = {}): string {
  const runtime = options.runtime ?? 'node'
  const shared = sharedServerSource(options, runtime)
  return runtime === 'node' ? `${shared}${nodeTransport()}` : `${shared}${fetchTransport(runtime)}`
}

/**
 * The half of the program that does not depend on the runtime.
 *
 * Which file a URL names, what it is served as, how long it may be cached,
 * which bytes a range asks for, whether the handler or the publish directory
 * answers first, and what a shutdown budget is. A transport below turns those
 * decisions into bytes on a socket and nothing more.
 */
function sharedServerSource(
  options: StandaloneServerOptions,
  runtime: StandaloneServerRuntime,
): string {
  const isrCacheDirectory =
    options.isrCache === 'tmp' ? "path.join(os.tmpdir(), 'ruvyxa-isr-cache')" : 'prerenderDir'

  return `import { createHandler, parseByteRange, prerenderRelativePath } from './serverless-handler.mjs';
import { applyPluginHttp, loadActionModule, loadRouteModule } from './route-modules.mjs';
// Imported so the directory stays deployable through any bundler that a host
// puts in front of it, matching the serverless adapters.
import manifest from './manifest.mjs';
import { readFileSync, writeFileSync, mkdirSync, statSync } from 'node:fs';
// Imported by specifier rather than taken from the global scope: \`process\` is a
// global on Node and on Bun, and on Deno only under its Node compatibility
// layer, which the specifier is what turns on.
import process from 'node:process';
import os from 'node:os';
import path from 'node:path';

const runtimePolicy = ${JSON.stringify(options.runtimePolicy ?? {})};

// The runtime this program was emitted for. Reported at startup because a
// deployment that is being served by a runtime nobody expected is otherwise
// indistinguishable from one that is.
const RUVYXA_RUNTIME = ${JSON.stringify(runtime)};

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

// True when the last path segment names a static asset file. Matches
// isStaticAssetPath in serverless-handler.mjs.
function isAssetPath(pathname) {
  const segment = pathname.slice(pathname.lastIndexOf('/') + 1);
  const dot = segment.lastIndexOf('.');
  if (dot <= 0 || dot === segment.length - 1) return false;
  return ASSET_EXTENSIONS.has(segment.slice(dot + 1).toLowerCase());
}

// Response compression, which \`ruvyxa dev\` and \`ruvyxa start\` have always
// done and this server did not: a self-hosted deployment sent every document
// and every client bundle uncompressed, so the production build shipped more
// bytes than the development server it was compared against. Set
// RUVYXA_COMPRESSION=0 where a proxy already compresses — doing it twice only
// spends CPU on the smaller machine.
const COMPRESSION_ENABLED = !['0', 'false', 'off'].includes(
  String(process.env.RUVYXA_COMPRESSION ?? '').trim().toLowerCase(),
);

// Below this, the framing and the CPU cost are worth more than the bytes saved.
// Set well under one MTU rather than at the 1 KB that reads like a round
// number: a 400-byte document that gzips to 250 still arrives in one packet
// either way, but a 900-byte one that does not compress costs a second.
// A streamed response has no declared length and is always compressed, because
// there is nothing to compare — that is the SSR and PPR case, where the payload
// is a document and worth encoding anyway.
const COMPRESSION_MIN_BYTES = 256;

// Text-shaped payloads only. Images, video, fonts, and archives are already
// compressed, and running them through gzip reliably makes them larger.
const COMPRESSIBLE_TYPE =
  /^(?:text\\/|application\\/(?:json|javascript|xml|xhtml\\+xml|rss\\+xml|atom\\+xml|ld\\+json|manifest\\+json|wasm)|image\\/svg\\+xml)/i;

function isCompressibleType(contentType) {
  return COMPRESSION_ENABLED && COMPRESSIBLE_TYPE.test(String(contentType ?? ''));
}

/**
 * The content codings a client will accept, as a token to q-value map.
 *
 * \`q=0\` is a refusal rather than a low preference, which is the part a naive
 * \`includes('gzip')\` gets wrong — and the Rust server already reads it that
 * way, so answering differently would make one project compress under
 * \`ruvyxa start\` and not under its own build.
 */
function acceptedEncodings(header) {
  const accepted = new Map();
  for (const part of String(header ?? '').split(',')) {
    const [rawToken, ...parameters] = part.split(';');
    const token = rawToken.trim().toLowerCase();
    if (token === '') continue;
    const quality = parameters
      .map((parameter) => parameter.trim().toLowerCase())
      .find((parameter) => parameter.startsWith('q='));
    accepted.set(token, quality === undefined ? 1 : Number(quality.slice(2)));
  }
  return accepted;
}

/**
 * Which coding to answer with, or null for none.
 *
 * Only what \`CompressionStream\` and \`node:zlib\` both implement, so the three
 * transports cannot diverge on the answer. Brotli is deliberately absent: it
 * has no \`CompressionStream\` format, so supporting it here would mean one
 * runtime compressing better than the other two for the same build.
 */
function negotiateEncoding(acceptEncoding) {
  const accepted = acceptedEncodings(acceptEncoding);
  for (const candidate of ['gzip', 'deflate']) {
    const quality = accepted.get(candidate) ?? accepted.get('*');
    if (quality !== undefined && !(quality === 0)) return candidate;
  }
  return null;
}

/**
 * The coding this response should be sent with, or null to send it as-is.
 *
 * One decision for all three transports and both paths — the static file and
 * the handler's response. A 206 and a 416 are excluded because their byte
 * offsets describe the identity encoding, and a body compressed underneath a
 * \`content-range\` is a range the client cannot use.
 */
function compressionFor(status, method, contentType, contentLength, contentEncoding, accept) {
  if (method === 'HEAD') return null;
  if (status === 204 || status === 206 || status === 304 || status === 416) return null;
  // Already encoded by whatever produced it; re-encoding would mislabel it.
  if (contentEncoding) return null;
  if (!isCompressibleType(contentType)) return null;
  if (contentLength !== null && contentLength < COMPRESSION_MIN_BYTES) return null;
  return negotiateEncoding(accept);
}

/** \`content-length\` as a number, or null when it is absent or unusable. */
function contentLengthOf(value) {
  const length = Number(value);
  return value === null || value === undefined || !Number.isFinite(length) ? null : length;
}

/** Add \`accept-encoding\` to a \`Vary\` value without dropping what is there. */
function withVaryAcceptEncoding(existing) {
  const present = String(existing ?? '')
    .split(',')
    .map((token) => token.trim())
    .filter((token) => token !== '');
  if (present.some((token) => token.toLowerCase() === 'accept-encoding')) {
    return present.join(', ');
  }
  return [...present, 'accept-encoding'].join(', ');
}

/**
 * Everything about serving one file except the sending of it.
 *
 * Status, every header, and which bytes are wanted, in one object. Reading
 * those bytes is the one part that is a runtime API rather than a decision, so
 * it is the one part a transport below is allowed to answer differently: a
 * second copy of this function is how two runtimes end up disagreeing about a
 * cache lifetime or a content type without anyone noticing.
 *
 * Returns null when no file matches, which is the caller's signal to fall
 * through to routing.
 */
function staticResponsePlan(pathname, rangeHeader) {
  const hit = resolveStaticFile(pathname);
  if (!hit) return null;

  // Ranges, decided by the same table the Rust server answers. This host and
  // \`ruvyxa start\` serve the same public/ directory, so a video that scrubs
  // under one has to scrub under the other.
  const range = parseByteRange(rangeHeader ?? '', hit.size);
  if (range.kind === 'unsatisfiable') {
    return {
      status: 416,
      headers: { 'accept-ranges': 'bytes', 'content-range': 'bytes */' + hit.size },
      file: hit.file,
      partial: null,
    };
  }
  const partial = range.kind === 'partial' ? { start: range.start, end: range.end } : null;

  // Keyed without the leading dot, and lowercased because a file system hands
  // back \`hero.PNG\` exactly as it was written.
  const contentType =
    MIME_TYPES[path.extname(hit.file).slice(1).toLowerCase()] ??
    ${JSON.stringify(FALLBACK_CONTENT_TYPE)};
  const headers = {
    'content-type': contentType,
    'accept-ranges': 'bytes',
    'content-length': String(partial ? partial.end - partial.start + 1 : hit.size),
    // Same cache policy the Rust server applies to the same files: hashed
    // bundles are immutable, everything else from public/ revalidates hourly
    // instead of on every navigation.
    'cache-control': pathname.startsWith(${JSON.stringify(CLIENT_BUNDLE_PREFIX)})
      ? ${JSON.stringify(IMMUTABLE_CACHE_CONTROL)}
      : ${JSON.stringify(PUBLIC_ASSET_CACHE_CONTROL)},
  };
  if (partial) {
    headers['content-range'] = 'bytes ' + partial.start + '-' + partial.end + '/' + hit.size;
  }
  // Declared for every compressible file, not only the ones actually
  // compressed: a shared cache that stored one client's gzip copy without this
  // would hand it to the next client whether or not that client can read it.
  if (isCompressibleType(contentType)) headers['vary'] = 'accept-encoding';
  return { status: partial ? 206 : 200, headers, file: hit.file, partial };
}

const port = Number(process.env.PORT || 3000);
const host = process.env.HOST || '0.0.0.0';

function positiveMs(name, fallback) {
  const raw = Number(process.env[name]);
  return Number.isFinite(raw) && raw > 0 ? raw : fallback;
}

// How long in-flight work may finish after a shutdown signal. Platforms send
// SIGTERM and then SIGKILL after their own grace period (commonly 30s), so
// this stays under the usual floor.
const SHUTDOWN_GRACE_MS = positiveMs('RUVYXA_SHUTDOWN_GRACE', 25_000);

// SIGTERM is what an orchestrator sends and SIGINT is what an operator sends.
// Registered one at a time inside a try: Deno on Windows supports only a subset
// of POSIX signals and throws on the rest, and losing SIGINT because SIGTERM
// could not be registered would leave the process unstoppable from a terminal.
function onShutdownSignal(handle) {
  for (const signal of ['SIGTERM', 'SIGINT']) {
    try {
      process.on(signal, () => handle(\`received \${signal}\`, 0));
    } catch {
      // this runtime and platform do not deliver that signal
    }
  }
}

`
}

/**
 * The Node transport: `node:http`, and the request and response objects it
 * gives us.
 *
 * Deliberately not written as a `fetch` handler wrapped in an adapter. On Node
 * that shape means every static file is read into a web stream and piped back
 * out through a Node one, and every response header is copied twice. The
 * transport is where a runtime's own strengths belong; the decisions above are
 * where they do not.
 */
function nodeTransport(): string {
  return `import { createServer } from 'node:http';
import { createReadStream } from 'node:fs';
import { Readable } from 'node:stream';
import { createDeflate, createGzip } from 'node:zlib';

/**
 * Insert the encoder for \`encoding\` between a body and the response.
 *
 * \`node:zlib\` rather than \`CompressionStream\` because this transport already
 * works in Node streams end to end, and routing a file through a web stream and
 * back is the copying the transport exists to avoid. The *decision* is shared
 * with the other two runtimes; only the mechanism is Node's.
 */
function pipeCompressed(source, res, encoding) {
  const encoder = encoding === 'gzip' ? createGzip() : createDeflate();
  // The status and headers are already committed, so a failure here can only
  // become a dropped connection — but unhandled it would take the process down.
  encoder.on('error', (error) => {
    console.error('[ruvyxa] response compression failed:', error);
    res.destroy(error);
  });
  res.on('close', () => encoder.destroy());
  source.pipe(encoder).pipe(res);
}

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

function sendStatic(req, res, plan) {
  res.statusCode = plan.status;
  for (const [name, value] of Object.entries(plan.headers)) res.setHeader(name, value);
  if (plan.status === 416 || req.method === 'HEAD') {
    res.end();
    return;
  }
  const encoding = compressionFor(
    plan.status,
    req.method,
    plan.headers['content-type'],
    contentLengthOf(plan.headers['content-length']),
    null,
    req.headers['accept-encoding'],
  );
  if (encoding) {
    res.setHeader('content-encoding', encoding);
    // The declared length describes the file, not the encoded bytes, and
    // nothing knows the encoded length until the stream ends.
    res.removeHeader('content-length');
  }
  const file = plan.partial
    ? createReadStream(plan.file, { start: plan.partial.start, end: plan.partial.end })
    : createReadStream(plan.file);
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
  if (encoding) {
    pipeCompressed(file, res, encoding);
    return;
  }
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
      const plan = staticResponsePlan(url.pathname, req.headers.range);
      if (plan) {
        sendStatic(req, res, plan);
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
      const plan = staticResponsePlan(url.pathname, req.headers.range);
      if (plan) {
        sendStatic(req, res, plan);
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

    const contentType = response.headers.get('content-type');
    if (isCompressibleType(contentType)) {
      res.setHeader('vary', withVaryAcceptEncoding(res.getHeader('vary')));
    }
    const encoding = compressionFor(
      response.status,
      req.method,
      contentType,
      contentLengthOf(response.headers.get('content-length')),
      response.headers.get('content-encoding'),
      req.headers['accept-encoding'],
    );
    if (encoding) {
      res.setHeader('content-encoding', encoding);
      res.removeHeader('content-length');
    }

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
    if (encoding) {
      pipeCompressed(body, res, encoding);
      return;
    }
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

onShutdownSignal(shutdown);

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
  console.log(\`[ruvyxa] \${RUVYXA_RUNTIME} standalone server listening on http://\${host === '0.0.0.0' ? 'localhost' : host}:\${port}\`);
});
`
}

/**
 * The Bun and Deno transport: their native servers, which already speak
 * `Request` and `Response`.
 *
 * `createHandler` *is* a `Request` → `Response` function, so on these two
 * runtimes there is nothing to adapt: no header list to rebuild, no web stream
 * to convert into a Node stream, no body to buffer before the handler can see
 * it. The only runtime-specific piece left is turning a file and a byte range
 * into a body, which is `openStaticBody` below and nothing else.
 */
function fetchTransport(runtime: 'bun' | 'deno'): string {
  const openStaticBody =
    runtime === 'bun'
      ? `// \`Bun.file\` hands the socket a file rather than a copy of its bytes, and a
// slice of one is still a file. This is the path Bun optimizes, and the slice is
// handed over *as a file* rather than as its \`.stream()\`: measured against Bun
// 1.4.0, a sliced \`BunFile\` read through \`.text()\`, \`.bytes()\`, or as a
// response body all give the window, while the same slice's \`.stream()\` served
// by \`Bun.serve\` sends the whole file — a 206 whose body is the entire video,
// which is what a seek would have played. Bun leaves a handler's own
// \`content-range\` and status alone, so nothing here has to work around its
// static-route range handling either.
function openStaticBody(plan) {
  const file = Bun.file(plan.file);
  return plan.partial ? file.slice(plan.partial.start, plan.partial.end + 1) : file;
}`
      : `// A whole file is Deno's own readable, which closes its handle when the
// response is finished or cancelled. A range is read through the Node
// compatibility stream instead: it takes the window as arguments, where
// \`Deno.open\` would need a seek and a limiter around a stream that does not
// know where to stop.
async function openStaticBody(plan) {
  if (plan.partial) {
    const { createReadStream } = await import('node:fs');
    const { Readable } = await import('node:stream');
    return Readable.toWeb(
      createReadStream(plan.file, { start: plan.partial.start, end: plan.partial.end }),
    );
  }
  return (await Deno.open(plan.file, { read: true })).readable;
}`

  const listen =
    runtime === 'bun'
      ? `// Bun retires an idle connection only when asked to: \`idleTimeout\` arrived in
// Bun 1.1.26 defaulting to ten seconds and has defaulted to 0 — never — since
// 1.1.27. Zero is already on the safe side of the 502 the Node transport raises
// \`keepAliveTimeout\` to avoid, and it is what lets a long streamed response
// stay open, so it is left alone unless an operator asks for a bound. Bun takes
// seconds where Node takes milliseconds, and caps at 255.
const idleSeconds = Number(process.env.RUVYXA_KEEP_ALIVE_TIMEOUT);
const idleTimeout = Number.isFinite(idleSeconds) && idleSeconds > 0
  ? { idleTimeout: Math.min(255, Math.max(1, Math.round(idleSeconds / 1000))) }
  : {};

const server = Bun.serve({
  port,
  hostname: host,
  ...idleTimeout,
  fetch: handleRequest,
  error: (error) => {
    console.error('[ruvyxa] request failed:', error instanceof Error ? error.message : error);
    return new Response('Internal Server Error', {
      status: 500,
      headers: { 'content-type': 'text/plain; charset=utf-8' },
    });
  },
});

// Stop accepting, let in-flight responses finish, and leave. \`stop(false)\`
// is the draining form; \`stop(true)\` would cut open responses off, which is
// the failure a graceful shutdown exists to prevent.
function closeServer() {
  return server.stop(false);
}`
      : `const server = Deno.serve(
  {
    port,
    hostname: host,
    onListen: ({ hostname, port: boundPort }) => {
      console.log(
        \`[ruvyxa] \${RUVYXA_RUNTIME} standalone server listening on http://\${hostname === '0.0.0.0' ? 'localhost' : hostname}:\${boundPort}\`,
      );
    },
    onError: (error) => {
      console.error('[ruvyxa] request failed:', error instanceof Error ? error.message : error);
      return new Response('Internal Server Error', {
        status: 500,
        headers: { 'content-type': 'text/plain; charset=utf-8' },
      });
    },
  },
  handleRequest,
);

// Deno's shutdown already waits for in-flight responses, which is exactly the
// drain the Node transport builds by hand.
function closeServer() {
  return server.shutdown();
}`

  const banner =
    runtime === 'bun'
      ? `console.log(
  \`[ruvyxa] \${RUVYXA_RUNTIME} standalone server listening on http://\${host === '0.0.0.0' ? 'localhost' : host}:\${port}\`,
);
`
      : ''

  return `${openStaticBody}

/**
 * Add the security defaults a response has not set for itself.
 *
 * Rebuilt rather than mutated, because \`Response.redirect\` produces immutable
 * headers and the handler returns one for every middleware redirect — setting a
 * header on that throws. \`Set-Cookie\` is carried across explicitly: it is the
 * one header that can legitimately appear more than once, and a session that
 * silently loses its second cookie is not a failure anyone sees in testing.
 */
function withSecurityHeaders(response) {
  if (!SECURITY_HEADERS_ENABLED) return response;
  const missing = Object.entries(DEFAULT_SECURITY_HEADERS).filter(
    ([name]) => !response.headers.has(name),
  );
  if (missing.length === 0) return response;
  const setCookies = response.headers.getSetCookie?.() ?? [];
  const headers = new Headers(response.headers);
  for (const [name, value] of missing) headers.set(name, value);
  if (setCookies.length > 0) {
    headers.delete('set-cookie');
    for (const cookie of setCookies) headers.append('set-cookie', cookie);
  }
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

async function staticResponse(plan, method) {
  const headers = new Headers(plan.headers);
  if (SECURITY_HEADERS_ENABLED) {
    for (const [name, value] of Object.entries(DEFAULT_SECURITY_HEADERS)) headers.set(name, value);
  }
  // A 416 carries no body, and a HEAD asks for the headers of the body it
  // would have received — including its \`content-length\`, which is why the
  // plan's headers are sent unchanged rather than recomputed for an empty one.
  if (plan.status === 416 || method === 'HEAD') {
    return new Response(null, { status: plan.status, headers });
  }
  return new Response(await openStaticBody(plan), { status: plan.status, headers });
}

/**
 * Encode the body if the client asked for it and the payload is worth it.
 *
 * \`CompressionStream\` rather than a runtime-specific encoder: it is the one
 * API both Bun and Deno implement, so these two transports cannot disagree
 * about what a build compresses. The response is rebuilt because its headers
 * may be immutable — every middleware redirect is a \`Response.redirect\` —
 * and \`Set-Cookie\` is carried across by hand for the reason
 * \`withSecurityHeaders\` gives just above.
 */
function withCompression(response, request) {
  const contentType = response.headers.get('content-type');
  if (!isCompressibleType(contentType)) return response;

  const encoding = compressionFor(
    response.status,
    request.method,
    contentType,
    contentLengthOf(response.headers.get('content-length')),
    response.headers.get('content-encoding'),
    request.headers.get('accept-encoding'),
  );

  if (!encoding) {
    // Nothing to encode, so the headers are adjusted in place rather than by
    // rebuilding. Constructing a new Response means taking \`response.body\`,
    // and on Bun taking the stream of a sliced \`BunFile\` yields the whole
    // file — which turned every range over a compressible asset into the
    // entire asset behind a 206.
    try {
      response.headers.set('vary', withVaryAcceptEncoding(response.headers.get('vary')));
    } catch {
      // \`Response.redirect\` has immutable headers, and it carries no body for
      // a cache to key wrong, so there is nothing lost by leaving it alone.
    }
    return response;
  }

  const setCookies = response.headers.getSetCookie?.() ?? [];
  const headers = new Headers(response.headers);
  headers.set('vary', withVaryAcceptEncoding(response.headers.get('vary')));
  if (setCookies.length > 0) {
    headers.delete('set-cookie');
    for (const cookie of setCookies) headers.append('set-cookie', cookie);
  }

  // Negotiation said yes but there is nothing to encode. The rebuild still
  // happens so \`Vary\` survives; \`content-encoding\` does not, because an
  // empty body is not gzip.
  if (response.body === null) {
    return new Response(null, {
      status: response.status,
      statusText: response.statusText,
      headers,
    });
  }

  headers.set('content-encoding', encoding);
  // Nothing knows the encoded length until the stream ends, and a stale one
  // would truncate the response at the client.
  headers.delete('content-length');
  return new Response(response.body.pipeThrough(new CompressionStream(encoding)), {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

async function handleRequest(request) {
  const url = new URL(request.url);
  const isRead = request.method === 'GET' || request.method === 'HEAD';

  // Hashed client bundles and asset-shaped paths are served before routing,
  // the order the Rust server uses. Page-shaped paths go through the handler
  // first so ISR revalidation and dynamic routes keep working; unmatched
  // paths fall back to static files.
  if (isRead && (url.pathname.startsWith('/__ruvyxa/') || isAssetPath(url.pathname))) {
    const plan = staticResponsePlan(url.pathname, request.headers.get('range'));
    if (plan) return withCompression(await staticResponse(plan, request.method), request);
  }

  // The request goes to the handler as it arrived. Its own \`security.apiLimit\`
  // check reads \`content-length\` and answers 413 before a body is consumed,
  // so there is nothing for this transport to buffer or bound — the Node one
  // buffers only because \`node:http\` gave it a stream and not a \`Request\`.
  const response = await handler(request);

  if (response.status === 404 && isRead) {
    const plan = staticResponsePlan(url.pathname, request.headers.get('range'));
    if (plan) return withCompression(await staticResponse(plan, request.method), request);
  }
  return withCompression(withSecurityHeaders(response), request);
}

${listen}

let shuttingDown = false;

async function shutdown(reason, exitCode) {
  if (shuttingDown) return;
  shuttingDown = true;
  console.log(\`[ruvyxa] \${reason}: draining connections\`);

  // A request that never finishes must not outlive the platform's own grace
  // period, or the process is SIGKILLed and the drain was pointless.
  const forceExit = setTimeout(() => {
    console.error('[ruvyxa] drain timed out; exiting with requests still open');
    process.exit(exitCode);
  }, SHUTDOWN_GRACE_MS);
  if (typeof forceExit?.unref === 'function') forceExit.unref();

  try {
    await closeServer();
  } catch (error) {
    console.error('[ruvyxa] shutdown failed:', error);
  }
  clearTimeout(forceExit);
  console.log('[ruvyxa] shutdown complete');
  process.exit(exitCode);
}

onShutdownSignal(shutdown);

// One route that rejects outside the request's own try/catch would otherwise
// terminate the process and take every other in-flight request down with it. A
// single bad request must not be able to do that, so this is reported and the
// server keeps serving.
process.on('unhandledRejection', (reason) => {
  console.error('[ruvyxa] unhandled promise rejection:', reason);
});

// An uncaught exception is different: the process state after it is undefined,
// so continuing to serve from it is not trustworthy. Drain what is in flight,
// then leave with a non-zero code so the supervisor restarts a clean process.
process.on('uncaughtException', (error) => {
  console.error('[ruvyxa] uncaught exception:', error);
  void shutdown('uncaught exception', 1);
});

${banner}`
}
