import {
  CLIENT_BUNDLE_PREFIX,
  DEFAULT_SECURITY_HEADERS,
  FALLBACK_CONTENT_TYPE,
  IMMUTABLE_CACHE_CONTROL,
  PUBLIC_ASSET_CACHE_CONTROL,
  STATIC_ASSET_EXTENSIONS,
  STATIC_CONTENT_TYPES,
  documentCacheOptionsSource,
  isrTemporaryCacheDirSource,
  platformDocumentStoreSource,
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
  /**
   * The build this program is being emitted from, as
   * `manifest.json`'s `deploy.buildId`.
   *
   * Only `isrCache: 'tmp'` reads it, and only to name the directory it writes
   * to: the host's temporary directory is shared with every other deployment
   * on the machine and with every previous build of this one. Absent, the
   * directory is still per-deployment — it is named from where the bundle was
   * deployed as well — but two builds deployed to the same path share it,
   * which is exactly a redeploy.
   */
  buildId?: string
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
  const temporaryIsrCache = options.isrCache === 'tmp'

  // A name nothing else on the host answers to.
  //
  // The reasoning, and the deployment it went wrong on, are on
  // [[isrTemporaryCacheDirSource]] rather than here: the three serverless
  // adapters emit the same expression, and this used to be the only one of the
  // four that was right. `here` is this host's spelling of the bundle's own
  // location, which is the half of the identity the build id does not supply.
  const isrCacheDirectory = temporaryIsrCache
    ? isrTemporaryCacheDirSource(options.buildId ?? '', 'here')
    : 'prerenderDir'

  // Created up front and owner-only, because the parent is mode 1777 on Linux:
  // anything may create a name there first, and a file or a symlink planted at
  // a route's cache path would be served as that page and written through on
  // the next refresh. Fail-soft on purpose — a host whose temporary directory
  // cannot be written to still serves every page, because an ordinary ISR write
  // that fails is caught by `persistPrerendered`. Making this fatal would turn a
  // degraded cache into a deployment that does not boot.
  const isrCacheSetup = temporaryIsrCache
    ? `
try {
  mkdirSync(isrCacheDir, { recursive: true, mode: 0o700 });
} catch (error) {
  log('warn', 'isr cache directory unavailable', {
    directory: isrCacheDir,
    error: error instanceof Error ? error.message : String(error),
  });
}

`
    : ''

  return `import { clientAddress, createHandler, logRecord, parseByteRange, parseTrustedProxies, prerenderRelativePath } from './serverless-handler.mjs';
import { applyPluginHttp, documentCacheHandler, loadActionModule, loadRouteModule } from './route-modules.mjs';
// The controller the render worker pool already runs on, reused rather than
// rewritten: bounded FIFO admission is one decision, and two implementations of
// it would be two overload behaviours for one framework.
import { WorkerAdmissionController } from './worker-admission.mjs';
// Imported so the directory stays deployable through any bundler that a host
// puts in front of it, matching the serverless adapters.
import manifest from './manifest.mjs';
import { readFileSync, realpathSync, writeFileSync, mkdirSync, statSync } from 'node:fs';${temporaryIsrCache ? "\n// Only the temporary ISR cache needs one: it is what names the directory.\nimport { createHash } from 'node:crypto';" : ''}
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

// The message stays \`listening on\` in both formats because it is a readiness
// signal, not prose: a supervisor, a container healthcheck script, and this
// repository's own conformance harness all wait for those words on stdout, and
// renaming it would leave every one of them waiting for twenty seconds and then
// giving up.
//
// One object per line for a collector, or the human shape for a terminal. The
// escaping is structural in the first and applied by hand in the second, so a
// value carrying a newline cannot become a record nobody wrote either way; see
// \`logValue\` in serverless-handler.mjs for the request that proved it could.
const LOG_FORMAT = process.env.RUVYXA_LOG_FORMAT === 'json' ? 'json' : 'text';
const log = (level, message, fields) => logRecord(LOG_FORMAT, level, message, fields ?? {});

const here = import.meta.dirname;
const prerenderDir = path.join(here, 'prerender');
const isrCacheDir = ${isrCacheDirectory};
${isrCacheSetup}const publicDir = path.resolve(here, '..', 'public');

${platformDocumentStoreSource()}

const handler = createHandler({
  routes: manifest.routes,
  middleware: runtimePolicy.middleware,
  i18n: manifest.i18n,
  importPage: loadRouteModule,
  importApi: loadRouteModule,
  importAction: loadActionModule,
  pluginHttp: applyPluginHttp,
  security: runtimePolicy.security,
  logFormat: LOG_FORMAT,
${documentCacheOptionsSource('platformReadPrerendered', 'platformWritePrerendered')}
  // The project's own not-found page, pre-rendered by the build and carried
  // inline in the manifest: an unmatched URL is answered with the page the
  // application actually wrote, on every host.
  notFoundDocument: manifest.notFoundDocument,
  supportedStrategies: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
});

// The headers that state who a request belongs to when something in front of
// this server overwrote them, and that state nothing at all when nothing did.
const FORWARDED_IDENTITY_HEADERS = ['x-forwarded-for', 'x-real-ip'];

const TRUSTED_PROXIES = parseTrustedProxies(runtimePolicy.security?.trustedProxyIps);

/**
 * Whether this connection's own peer may state who the client is.
 *
 * \`createHandler\` scans \`X-Forwarded-For\` from the right and has no peer to
 * weigh it against — a deployed function does not have one. This is a socket
 * server and does: \`ruvyxa start\` has always gated the forwarded chain on the
 * transport peer, and this program did not, so every self-hosted deployment
 * reachable without a header-overwriting proxy in front of it believed whatever
 * the caller typed. One client rotating the header collected a fresh bucket per
 * request from the built-in \`rate\` middleware, the server-action rate limiter,
 * and the action replay guard's per-client quota at once, and poisoned the
 * \`client\` field in the request log so the abuse was invisible afterwards.
 *
 * The rule itself is \`clientAddress\` from serverless-handler.mjs, asked rather
 * than restated: it attributes a request to the rightmost hop that is *not* a
 * trusted proxy, so a peer it declines to attribute is one. A second copy of
 * the prefix matcher here would be a second answer to who may speak for a
 * client, which is the shape this whole module exists to avoid.
 *
 * Loopback is trusted without configuration, matching the native host: a proxy
 * terminating on the same host is the ordinary deployment. Anything else has to
 * be named in \`security.trustedProxyIps\`.
 */
function peerMayStateClientIdentity(peer) {
  if (typeof peer !== 'string' || peer.trim() === '') return false;
  // \`parseTrustedProxies\` drops whatever is not an address, so a one-entry
  // result is the proof that the peer parsed as one. Without this a peer that
  // is not an address at all — a closed socket, a Unix domain socket — would
  // reach the check below, be attributed to nobody, and read as trusted.
  if (parseTrustedProxies([peer]).length !== 1) return false;
  return clientAddress(new Headers({ 'x-forwarded-for': peer }), TRUSTED_PROXIES) === 'unknown';
}

// Serialized from STATIC_CONTENT_TYPES so this server and the Rust one answer
// from the same table; see tests/fixtures/static-asset-conformance.json.
const MIME_TYPES = ${JSON.stringify(STATIC_CONTENT_TYPES)};

/**
 * The name a file answers to, so two spellings of one file count once.
 *
 * \`resolve_public_asset\` compares canonicalized paths for the same reason: on a
 * case-insensitive file system \`logo.png\` and \`logo.PNG\` name one file, and an
 * ambiguity check that counted them twice would refuse a project that only has
 * one image. Used as a key and never as a path to open, so a symlink cannot
 * widen what this server is willing to serve.
 */
function realFilePath(candidate) {
  try {
    return typeof realpathSync.native === 'function'
      ? realpathSync.native(candidate)
      : realpathSync(candidate);
  } catch {
    return candidate;
  }
}

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
  // And the other direction, which \`resolve_public_asset\` mirrors and this
  // server did not. \`<Image>\` rewrites every source to \`webpUrl(src)\`
  // unconditionally — it has no access to \`image.optimize\` — and
  // \`image.optimize: false\` publishes the source untouched with no \`.webp\`
  // beside it. Without this, every \`<Image>\` on the page 404s on every
  // self-hosted deployment of such a project, invisibly, because \`ruvyxa dev\`
  // and \`ruvyxa start\` both resolve it. The same applies to any source the
  // optimizer skipped.
  //
  // Exactly one source may answer, which is the Rust guard's rule: \`logo.png\`
  // and \`logo.jpg\` beside each other is the collision the build refuses, and a
  // first-hit loop would resolve it by array order and make the two hosts
  // disagree about the same publish directory. Upper-case spellings are
  // candidates because a case-sensitive file system stores \`hero.PNG\` under
  // that name, and are counted once because a case-insensitive one does not.
  if (/\\.webp$/i.test(resolved)) {
    const sources = new Map();
    for (const extension of ['png', 'jpg', 'jpeg', 'PNG', 'JPG', 'JPEG']) {
      const candidate = resolved.replace(/\\.webp$/i, '.' + extension);
      try {
        if (statSync(candidate).isFile()) sources.set(realFilePath(candidate), candidate);
      } catch {
        // no source published under that extension
      }
    }
    if (sources.size === 1) candidates.push(...sources.values());
  }
  for (const candidate of candidates) {
    try {
      const stats = statSync(candidate);
      if (stats.isFile()) return { file: candidate, size: stats.size, modified: stats.mtimeMs };
    } catch {
      // try the next candidate
    }
  }
  return null;
}

/**
 * The validator a revalidation of this file is answered against.
 *
 * Without one, \`cache-control: public, max-age=3600, must-revalidate\` on a
 * public asset is a promise this server cannot keep: the browser comes back
 * after the hour with nothing to ask about, so every image, font, and video is
 * re-sent in full. \`ruvyxa start\` has always answered the same file with an
 * ETag and a 304, so a project got smaller responses from the development
 * command than from its own production build.
 *
 * Weak, and from size and mtime rather than a content hash, for two reasons.
 * The same file is served identity or gzipped depending on what the client
 * accepts, and those are equivalent representations rather than byte-identical
 * ones — which is what a weak validator states. And this server has no
 * fingerprint index in front of it, so hashing a file to decide whether to send
 * it is exactly the work a 304 exists to avoid.
 */
function fileValidator(hit) {
  const modified = Math.floor(hit.modified / 1000);
  return {
    etag: 'W/"' + hit.size.toString(16) + '-' + modified.toString(16) + '"',
    lastModified: new Date(modified * 1000).toUTCString(),
    modified,
  };
}

/**
 * Whether \`if-none-match\` names the version the client already holds.
 *
 * Compared weakly, which is what a revalidation asks for: a \`W/\` prefix on
 * either side is ignored rather than making the comparison fail, and \`*\`
 * matches any existing representation.
 */
function matchesEtag(header, etag) {
  const value = String(header ?? '').trim();
  if (value === '') return false;
  if (value === '*') return true;
  const bare = (candidate) => candidate.trim().replace(/^W\\//, '');
  return value.split(',').some((candidate) => bare(candidate) === bare(etag));
}

/** Whether \`if-modified-since\` already covers this file's modification time. */
function notModifiedSince(header, modified) {
  if (typeof header !== 'string' || header.trim() === '') return false;
  const since = Date.parse(header);
  // Seconds, because that is the resolution the header carries: comparing a
  // millisecond mtime against a second-resolution date makes every file look
  // newer than the copy the client just received.
  return Number.isFinite(since) && modified <= Math.floor(since / 1000);
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

/**
 * The types that are never encoded, refused ahead of the allow-list above.
 *
 * The allow-list is a prefix regex, and \`^text\\/\` swallows the one text type
 * that must never be buffered. An SSE response also has no declared length, so
 * the size floor is waived for it too — and Node's default-flush \`gzip\` and
 * \`CompressionStream\` both hold a small write until roughly 16 KB has
 * accumulated or the stream ends. An \`EventSource\` against a self-hosted
 * deployment therefore received nothing while the same route streamed normally
 * under \`ruvyxa start\`, whose tower-http \`DefaultPredicate\` excludes this type.
 * \`application/grpc\` is excluded for the same reason and by the same predicate.
 */
const NON_COMPRESSIBLE_TYPE = /^(?:text\\/event-stream|application\\/grpc)/i;

/**
 * \`no-transform\` as a directive rather than as a substring, so it cannot be
 * matched inside some other directive's value.
 */
const NO_TRANSFORM = /(?:^|,)\\s*no-transform\\s*(?:$|,|;)/i;

/**
 * Whether this payload may be encoded at all.
 *
 * Answers the \`Vary\` question as well as the compression one, because they are
 * the same question: a response that will never be encoded must not advertise a
 * variance it does not have.
 */
function isCompressibleType(contentType, cacheControl) {
  if (!COMPRESSION_ENABLED) return false;
  const type = String(contentType ?? '');
  if (NON_COMPRESSIBLE_TYPE.test(type)) return false;
  // The header an application has to say "do not re-encode this" with, which
  // neither host read.
  if (NO_TRANSFORM.test(String(cacheControl ?? ''))) return false;
  return COMPRESSIBLE_TYPE.test(type);
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
function compressionFor(
  status,
  method,
  contentType,
  contentLength,
  contentEncoding,
  cacheControl,
  accept,
) {
  if (method === 'HEAD') return null;
  if (status === 204 || status === 206 || status === 304 || status === 416) return null;
  // Already encoded by whatever produced it; re-encoding would mislabel it.
  if (contentEncoding) return null;
  if (!isCompressibleType(contentType, cacheControl)) return null;
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
function staticResponsePlan(pathname, rangeHeader, conditional = {}) {
  const hit = resolveStaticFile(pathname);
  if (!hit) return null;

  const validator = fileValidator(hit);
  const cacheControl = pathname.startsWith(${JSON.stringify(CLIENT_BUNDLE_PREFIX)})
    ? ${JSON.stringify(IMMUTABLE_CACHE_CONTROL)}
    : ${JSON.stringify(PUBLIC_ASSET_CACHE_CONTROL)};

  // A revalidation is answered before the range is parsed and before the file
  // is opened: a 304 carries no body, so nothing below it has any work to do.
  // \`if-none-match\` wins over \`if-modified-since\` when both are present, which
  // is what RFC 9110 asks of a server that understands both.
  const noneMatch = conditional.ifNoneMatch;
  const fresh =
    typeof noneMatch === 'string' && noneMatch.trim() !== ''
      ? matchesEtag(noneMatch, validator.etag)
      : notModifiedSince(conditional.ifModifiedSince, validator.modified);
  if (fresh) {
    return {
      // No \`content-length\`: it describes the body a 200 would have carried,
      // and declaring it beside an empty one is a framing error the client
      // reads as a truncated response.
      status: 304,
      headers: {
        etag: validator.etag,
        'last-modified': validator.lastModified,
        'cache-control': cacheControl,
      },
      file: hit.file,
      partial: null,
    };
  }

  // Ranges, decided by the same table the Rust server answers. This host and
  // \`ruvyxa start\` serve the same public/ directory, so a video that scrubs
  // under one has to scrub under the other.
  // \`if-range\` makes the range conditional on the client still holding the
  // representation it is continuing. Without it a resumed download assembled
  // bytes from two different versions of the file into one corrupt result --
  // and this server sends both an entity tag and a \`last-modified\`, so clients
  // do send it. A mismatch is not an error: the whole file is the correct
  // answer, and it is how the client learns the file changed underneath it.
  // Same rule as \`requested_range\` on the native host, which serves the same
  // public/ directory.
  const ifRange = String(conditional.ifRange ?? '').trim();
  const rangeStillApplies =
    ifRange === ''
      ? true
      : ifRange.startsWith('"') || ifRange.startsWith('W/')
        ? ifRange.replace(/^W\\//, '') === validator.etag.replace(/^W\\//, '')
        : Date.parse(ifRange) === validator.modified * 1000;
  const range = rangeStillApplies
    ? parseByteRange(rangeHeader ?? '', hit.size)
    : { kind: 'whole' };
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
    'cache-control': cacheControl,
    // What that revalidation is answered against, on the 200 as well as the
    // 206: a client that resumes a download compares the two.
    etag: validator.etag,
    'last-modified': validator.lastModified,
  };
  if (partial) {
    headers['content-range'] = 'bytes ' + partial.start + '-' + partial.end + '/' + hit.size;
  }
  // Declared for every compressible file, not only the ones actually
  // compressed: a shared cache that stored one client's gzip copy without this
  // would hand it to the next client whether or not that client can read it.
  if (isCompressibleType(contentType, cacheControl)) headers['vary'] = 'accept-encoding';
  return {
    status: partial ? 206 : 200,
    headers,
    file: hit.file,
    partial,
    // A range was asked for and refused, because \`if-range\` named a version
    // this is no longer serving. Recorded rather than inferred downstream:
    // the Bun transport has to know, for the reason written above
    // \`openStaticBody\` there.
    declinedRange: !rangeStillApplies && String(rangeHeader ?? '').trim() !== '',
  };
}

const port = Number(process.env.PORT || 3000);
const host = process.env.HOST || '0.0.0.0';

function positiveNumber(name, fallback) {
  const raw = Number(process.env[name]);
  return Number.isFinite(raw) && raw > 0 ? raw : fallback;
}

// How long in-flight work may finish after a shutdown signal. Platforms send
// SIGTERM and then SIGKILL after their own grace period (commonly 30s), so
// this stays under the usual floor.
const SHUTDOWN_GRACE_MS = positiveNumber('RUVYXA_SHUTDOWN_GRACE', 25_000);

/**
 * How long the process keeps listening, and serving, after a shutdown signal
 * before it stops accepting new connections.
 *
 * Closing the socket the instant the signal lands makes the draining status
 * unreachable: a readiness probe opens a new connection and is refused, so the
 * one answer that tells an orchestrator to stop routing here never arrives.
 * Everything it sends while it is still deregistering then fails in a browser
 * instead of being retried against another instance — which is the failure a
 * drain exists to prevent, and it happened on every self-hosted deployment.
 *
 * The process serves normally for the whole window; only the readiness answer
 * changes. Capped at half of SHUTDOWN_GRACE_MS so in-flight work keeps a budget
 * of its own however the two are configured, and RUVYXA_DRAIN_DELAY=0 closes
 * straight away, which is right where nothing is load-balancing this process.
 */
const DRAIN_DELAY_MS = Math.min(
  process.env.RUVYXA_DRAIN_DELAY?.trim() === '0'
    ? 0
    : positiveNumber('RUVYXA_DRAIN_DELAY', 5_000),
  SHUTDOWN_GRACE_MS / 2,
);

/**
 * How long the handler may take to produce a response.
 *
 * A route whose render never settles has nothing to stop it here. On
 * \`ruvyxa start\` the same render is a worker round-trip the native host bounds
 * at \`RUVYXA_WORKER_TIMEOUT_MS\` (30s), and on a serverless adapter the platform
 * bounds the invocation — this is the only host where a hung render holds its
 * connection, its memory, and whatever it was waiting on for as long as the
 * process lives. One such route under ordinary traffic is a slow leak that ends
 * as an out-of-memory kill with no error in the log.
 *
 * The default matches the native host so one project is bounded the same way
 * under \`ruvyxa start\` and under its own build. \`RUVYXA_RENDER_TIMEOUT=0\` turns
 * it off for a deployment that genuinely renders longer than this and knows it.
 */
const RENDER_TIMEOUT_MS = process.env.RUVYXA_RENDER_TIMEOUT?.trim() === '0'
  ? 0
  : positiveNumber('RUVYXA_RENDER_TIMEOUT', 30_000);

/**
 * Run the handler, and give up on it after {@link RENDER_TIMEOUT_MS}.
 *
 * Bounds the wait for the \`Response\`, never the body behind it. A streamed
 * document and a server-sent-event stream both resolve their \`Response\` as soon
 * as the headers are known and then write for as long as they like, so this
 * cannot cut one short — which is why it is a render timeout and not a request
 * timeout.
 *
 * The handler is not cancellable, so the losing promise keeps running; its
 * rejection is swallowed here rather than left to surface later as an unhandled
 * rejection about a request nobody is waiting for any more.
 *
 * \`503\` rather than \`500\`: the render did not fail, this server stopped
 * waiting for it, and a caller that retries may well be served. \`Retry-After\`
 * says so in the one place a proxy reads.
 */
/**
 * How many renders may run at once, and how many may wait.
 *
 * Nothing bounded this. Every request that arrived got a render started for it,
 * so a burst larger than the machine turned into a heap holding every in-flight
 * render at once — and the failure is not a slow server, it is an
 * out-of-memory kill that takes the requests already nearly finished down with
 * the ones that caused it. \`ruvyxa start\` has never had that problem: its
 * render worker bounds itself with this same controller.
 *
 * The width is the core count rather than a large fixed number, because a render
 * is CPU-bound and admitting more than the machine can run only slows down the
 * ones already going. Four waiters per slot absorbs an ordinary burst; past that
 * a caller is told to come back rather than being parked on memory this process
 * would have to keep. Both are the worker pool's numbers, for the worker pool's
 * reasons.
 *
 * \`RUVYXA_MAX_CONCURRENCY=0\` turns admission off for a deployment that has
 * something else in front of it doing this.
 */
const MAX_CONCURRENT_RENDERS = process.env.RUVYXA_MAX_CONCURRENCY?.trim() === '0'
  ? 0
  : positiveNumber(
      'RUVYXA_MAX_CONCURRENCY',
      // \`availableParallelism\` is the container's share where it exists, which
      // is the number that matters; \`cpus()\` reports the host's on some of them.
      Math.max(2, Math.min(8, (os.availableParallelism?.() ?? os.cpus().length) || 2)),
    );
const MAX_QUEUED_RENDERS = positiveNumber('RUVYXA_MAX_QUEUE', MAX_CONCURRENT_RENDERS * 4 || 1);

const admission =
  MAX_CONCURRENT_RENDERS === 0
    ? null
    : new WorkerAdmissionController({
        maxConcurrentRequests: Math.trunc(MAX_CONCURRENT_RENDERS),
        maxQueuedRequests: Math.trunc(MAX_QUEUED_RENDERS),
      });

/**
 * Liveness and readiness for this process.
 *
 * Answered before routing and before admission, because a probe must not queue
 * behind the renders it exists to report on: a server whose health check waits
 * for a slot reports "unhealthy" exactly when it is merely busy, and the
 * orchestrator restarts a process that was working.
 *
 * \`200\` while serving and \`503\` once a drain has begun. That ordering is the
 * point — an orchestrator still routing to a process that has stopped accepting
 * sends it work it can only refuse, and this is the only thing that tells it in
 * time.
 *
 * Deliberately incurious: a status and the runtime that answered, nothing more.
 * This is a public path on a deployed server, and in-flight counts and queue
 * depth are a load oracle for anyone willing to ask often enough. The native
 * host answers the same path the same way; the two are held to
 * \`tests/fixtures/framework-endpoint-conformance.json\`.
 */
const HEALTH_PATH = '/__ruvyxa/health';

/**
 * Prometheus text exposition, off unless an operator turns it on.
 *
 * These are the numbers \`/__ruvyxa/health\` deliberately withholds — how many
 * renders are running, how deep the queue is, how many callers have been
 * refused. That is a load oracle for anyone willing to ask often enough, which
 * is why it is behind a bearer token rather than beside the public probe, and
 * why an unset token answers \`404\` rather than \`401\`: a deployment that never
 * turned metrics on should not advertise that the path exists.
 */
const METRICS_PATH = '/__ruvyxa/metrics';
const METRICS_TOKEN = (process.env.RUVYXA_METRICS_TOKEN ?? '').trim();
const STARTED_AT = Date.now();
let renderTimeouts = 0;

/**
 * Compare a presented token with the configured one without leaking where they
 * first differ.
 *
 * A \`===\` on secrets returns as soon as two bytes disagree, so the time it
 * takes says how long a guessed prefix was — enough to recover a token one byte
 * at a time from a few thousand requests. Length is compared first and does
 * leak, which is the accepted shape of this check everywhere it appears.
 */
function tokenMatches(presented) {
  if (presented.length !== METRICS_TOKEN.length) return false;
  let difference = 0;
  for (let index = 0; index < presented.length; index += 1) {
    difference |= presented.charCodeAt(index) ^ METRICS_TOKEN.charCodeAt(index);
  }
  return difference === 0;
}

/**
 * Takes a method and a credential rather than a \`Request\`, the way
 * \`healthResponse\` takes a method.
 *
 * The Node transport has no \`Request\` at this point and used to build one to
 * ask — which was harmless while only the header was read and stopped being so
 * once the verb mattered: \`new Request(url, { method })\` throws a \`TypeError\`
 * for CONNECT and TRACE, so a probe using either would have become a 500 on
 * that transport and a 405 on the other two.
 */
function metricsResponse(method, authorization) {
  // Not configured: the path does not exist, as far as anyone asking can tell.
  // Checked before the verb, so an unset token answers a POST the same 404 it
  // answers a GET rather than admitting the path is there to be scraped.
  if (METRICS_TOKEN === '') return null;

  // The path exists and this verb does not, which is the same thing
  // \`/__ruvyxa/health\` answers 405 to say. Falling through to routing told an
  // operator who had pointed a scraper at it with the wrong method that the
  // endpoint was never deployed. Before the token check on purpose: an
  // unauthorised GET already answers 401, so a 405 here reveals nothing a
  // caller could not already learn, and answering 401 to a verb this endpoint
  // will never serve would send them looking for a credential instead.
  if (method !== 'GET' && method !== 'HEAD') {
    return new Response('Method Not Allowed', {
      status: 405,
      headers: {
        allow: 'GET, HEAD',
        'content-type': 'text/plain; charset=utf-8',
        'cache-control': 'no-store',
      },
    });
  }

  const presented = authorization.startsWith('Bearer ') ? authorization.slice(7).trim() : '';
  if (!tokenMatches(presented)) {
    return new Response('Unauthorized', {
      status: 401,
      headers: {
        'www-authenticate': 'Bearer',
        'content-type': 'text/plain; charset=utf-8',
        'cache-control': 'no-store',
      },
    });
  }

  const snapshot = admission?.snapshot() ?? null;
  const lines = [
    '# HELP ruvyxa_build_info The runtime this deployment was emitted for.',
    '# TYPE ruvyxa_build_info gauge',
    \`ruvyxa_build_info{runtime="\${RUVYXA_RUNTIME}"} 1\`,
    '# HELP ruvyxa_uptime_seconds Seconds since this process began serving.',
    '# TYPE ruvyxa_uptime_seconds gauge',
    \`ruvyxa_uptime_seconds \${Math.floor((Date.now() - STARTED_AT) / 1000)}\`,
    '# HELP ruvyxa_render_timeouts_total Renders abandoned at RUVYXA_RENDER_TIMEOUT.',
    '# TYPE ruvyxa_render_timeouts_total counter',
    \`ruvyxa_render_timeouts_total \${renderTimeouts}\`,
    '# HELP ruvyxa_draining Whether a shutdown signal has arrived.',
    '# TYPE ruvyxa_draining gauge',
    \`ruvyxa_draining \${shuttingDown ? 1 : 0}\`,
  ];
  // Absent rather than zero when admission is off: a scrape that reported
  // "0 renders running, 0 queued" for a server with no limiter would read as a
  // healthy idle process rather than as an unbounded one.
  if (snapshot) {
    lines.push(
      '# HELP ruvyxa_renders_active Renders holding a slot right now.',
      '# TYPE ruvyxa_renders_active gauge',
      \`ruvyxa_renders_active \${snapshot.activeRequests}\`,
      '# HELP ruvyxa_renders_queued Requests waiting for a slot.',
      '# TYPE ruvyxa_renders_queued gauge',
      \`ruvyxa_renders_queued \${snapshot.queuedRequests}\`,
      '# HELP ruvyxa_renders_max_concurrent Configured render concurrency.',
      '# TYPE ruvyxa_renders_max_concurrent gauge',
      \`ruvyxa_renders_max_concurrent \${snapshot.maxConcurrentRequests}\`,
      '# HELP ruvyxa_renders_max_queued Configured queue depth.',
      '# TYPE ruvyxa_renders_max_queued gauge',
      \`ruvyxa_renders_max_queued \${snapshot.maxQueuedRequests}\`,
      '# HELP ruvyxa_renders_rejected_total Requests refused because the queue was full.',
      '# TYPE ruvyxa_renders_rejected_total counter',
      \`ruvyxa_renders_rejected_total \${snapshot.rejectedRequests}\`,
    );
  }
  return new Response(lines.join('\\n') + '\\n', {
    status: 200,
    headers: {
      'content-type': 'text/plain; version=0.0.4; charset=utf-8',
      'cache-control': 'no-store',
    },
  });
}

function healthResponse(method) {
  // The path exists; this verb does not. A 404 here would say the endpoint is
  // absent, which is what the native host answers 405 to avoid — see the
  // method-dispatch rules the three hosts already share.
  if (method !== 'GET' && method !== 'HEAD') {
    return new Response('Method Not Allowed', {
      status: 405,
      headers: {
        allow: 'GET, HEAD',
        'content-type': 'text/plain; charset=utf-8',
        'cache-control': 'no-store',
      },
    });
  }
  const status = shuttingDown ? 503 : 200;
  const headers = {
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
  };
  if (shuttingDown) headers['retry-after'] = '1';
  const body =
    JSON.stringify({ status: shuttingDown ? 'draining' : 'ok', host: RUVYXA_RUNTIME }) + '\\n';
  // A HEAD asks for the headers of the body it would have received, so the
  // status still says whether this process is taking traffic.
  return new Response(method === 'HEAD' ? null : body, { status, headers });
}

/** The answer to a request this server has decided not to start. */
function overloaded(reason) {
  log('warn', 'refused', { reason });
  return new Response('Service Unavailable', {
    status: 503,
    headers: {
      'content-type': 'text/plain; charset=utf-8',
      'retry-after': '1',
      'cache-control': 'no-store',
    },
  });
}

/**
 * Take a render slot, build the request, run the handler, and give the slot
 * back.
 *
 * The request is built *inside* the slot rather than handed in already made,
 * because on the Node transport making one means reading the body off the
 * socket: \`node:http\` gives a stream and \`new Request\` needs bytes. That read
 * ran before admission, so the controller did not bound the one path in this
 * program that allocates per request — 200 concurrent uploads were 200 buffers
 * on the heap, while the Bun and Deno deployments of the same artifact refused
 * them after four. What is admitted is the work, not the \`Request\`.
 *
 * The slot is released when the **response** exists, not when its body has
 * finished — the same boundary the render deadline uses, and for a sharper
 * reason here: a server-sent-event stream holds its body open for hours, and a
 * slot held that long would take the pool down to nothing after a handful of
 * subscribers. What is being bounded is the render, which is the part that
 * competes for the CPU.
 *
 * Only the handler is admitted. A static file is answered before this and stays
 * answered under load, so a page that is failing does not take its own
 * stylesheet down with it.
 */
async function handleAdmitted(buildRequest, pathname) {
  if (!admission) return handleWithTimeout(await buildRequest());
  const admitted = await admission.acquire();
  if (!admitted) {
    // Either the queue is full or the server is draining. Both mean the same
    // thing to the caller, and \`Retry-After\` is true of both. Nothing has been
    // read off the socket yet, so refusing here costs one response and no
    // memory.
    return overloaded(
      \`refused \${pathname}: \${MAX_CONCURRENT_RENDERS} renders running and \${MAX_QUEUED_RENDERS} waiting\`,
    );
  }
  try {
    return await handleWithTimeout(await buildRequest());
  } finally {
    admission.release();
  }
}

const RENDER_TIMED_OUT = Symbol('ruvyxa.renderTimedOut');

async function handleWithTimeout(request) {
  if (RENDER_TIMEOUT_MS === 0) return handler(request);

  let expire;
  const deadline = new Promise((resolve) => {
    expire = setTimeout(() => resolve(RENDER_TIMED_OUT), RENDER_TIMEOUT_MS);
  });
  // A rejection is carried rather than thrown, so that losing the race cannot
  // leave it unhandled: the request that would have caught it has already been
  // answered by the time it arrives.
  const rendered = handler(request).then(
    (response) => ({ response }),
    (error) => ({ error }),
  );

  try {
    const settled = await Promise.race([rendered, deadline]);
    if (settled === RENDER_TIMED_OUT) {
      renderTimeouts += 1;
      return overloaded(
        \`render timed out after \${RENDER_TIMEOUT_MS}ms: \${new URL(request.url).pathname}\`,
      );
    }
    if ('error' in settled) throw settled.error;
    return settled.response;
  } finally {
    clearTimeout(expire);
  }
}

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
    log('error', 'response compression failed', { error: String(error) });
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

// The largest body any endpoint of this deployment may accept.
//
// The body is buffered here before the handler sees it, so the handler's own
// cap would arrive too late to prevent the allocation — this is where the
// number has to bound the read. What it must *not* do is decide the policy:
// that belongs to \`requestBodyPolicy\` in \`serverless-handler.mjs\`, which is
// per endpoint. \`/__ruvyxa/action\` is bounded by \`security.actionLimit\` and
// \`/__ruvyxa/rsc\` by a fixed \`RSC_ACTION_BODY_LIMIT\`, neither of which a
// project's \`security.apiLimit\` speaks for.
//
// Deriving this from \`apiLimit\` alone meant a project that lowered the bound on
// its own API routes also lowered the transport under an endpoint the framework
// allows: a server-function call sized between the two was answered 413 here,
// before the endpoint that would have accepted it ever ran — and the same
// artifact accepted that call under Bun and Deno, which hand the handler the
// runtime's own \`Request\` and never buffer.
//
// So: the maximum of what any endpoint may allow, which bounds the allocation
// without overriding anyone's limit — every request still meets its own, one
// layer in. What bounds the *number* of buffers is admission, which is why the
// read happens inside \`handleAdmitted\` and not before it.
const RSC_ACTION_BODY_LIMIT = 4 * 1024 * 1024;
const configuredLimit = (value, fallback) =>
  Number.isInteger(value) && value > 0 ? value : fallback;
const REQUEST_BODY_LIMIT = Math.max(
  configuredLimit(runtimePolicy.security?.apiLimit, 10 * 1024 * 1024),
  configuredLimit(runtimePolicy.security?.actionLimit, 1024 * 1024),
  RSC_ACTION_BODY_LIMIT,
);

function sendStatic(req, res, plan) {
  res.statusCode = plan.status;
  for (const [name, value] of Object.entries(plan.headers)) res.setHeader(name, value);
  if (plan.status === 416 || plan.status === 304 || req.method === 'HEAD') {
    res.end();
    return;
  }
  const encoding = compressionFor(
    plan.status,
    req.method,
    plan.headers['content-type'],
    contentLengthOf(plan.headers['content-length']),
    null,
    plan.headers['cache-control'],
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
    log('error', 'static read failed', { error: String(error) });
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

    // Ahead of everything: a probe that queues behind the renders it exists to
    // report on says "unhealthy" when the server is merely busy.
    if (url.pathname === HEALTH_PATH) {
      const health = healthResponse(req.method);
      res.statusCode = health.status;
      for (const [key, value] of health.headers.entries()) res.setHeader(key, value);
      res.end(req.method === 'HEAD' ? undefined : await health.text());
      return;
    }
    if (url.pathname === METRICS_PATH) {
      // A null answer means no token is configured, so the path falls through
      // to routing and answers whatever an unknown URL answers. Every verb is
      // offered, because the 405 for the ones this endpoint does not serve is
      // decided there rather than by which requests reach it.
      const metrics = metricsResponse(req.method, req.headers.authorization ?? '');
      if (metrics) {
        res.statusCode = metrics.status;
        for (const [key, value] of metrics.headers.entries()) res.setHeader(key, value);
        res.end(req.method === 'HEAD' ? undefined : await metrics.text());
        return;
      }
    }

    // Hashed client bundles and asset-shaped paths are served before routing,
    // the order the Rust server uses. Page-shaped paths go through the handler
    // first so ISR revalidation and dynamic routes keep working; unmatched
    // paths fall back to static files.
    if (isRead && (url.pathname.startsWith('/__ruvyxa/') || isAssetPath(url.pathname))) {
      const plan = staticResponsePlan(url.pathname, req.headers.range, {
        ifNoneMatch: req.headers['if-none-match'],
        ifModifiedSince: req.headers['if-modified-since'],
        ifRange: req.headers['if-range'],
      });
      if (plan) {
        sendStatic(req, res, plan);
        return;
      }
    }

    // The trust decision is made here, where the peer exists, and the headers
    // an untrusted peer wrote are dropped rather than weighed: with them gone
    // \`clientAddress\` falls through to \`unknown\`, which buckets more
    // aggressively than the traffic warrants — the direction a limiter is
    // allowed to be wrong in. \`node:http\` has already lowercased every field
    // name and joined repeated field lines with \`", "\`. The peer is weighed
    // only when there is something to weigh it against, so a request that
    // carries neither header pays two property lookups.
    const forwardedAllowed =
      !FORWARDED_IDENTITY_HEADERS.some((name) => req.headers[name] !== undefined) ||
      peerMayStateClientIdentity(req.socket?.remoteAddress);
    const headers = new Headers();
    for (const [key, value] of Object.entries(req.headers)) {
      if (!forwardedAllowed && FORWARDED_IDENTITY_HEADERS.includes(key)) continue;
      if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
    }
    // The body is read with a slot in hand, which is what makes
    // MAX_CONCURRENT_RENDERS bound it: reading first meant a burst larger than
    // the machine became a heap holding every upload at once, before anything
    // had asked whether there was anywhere to put them. A caller refused here
    // is refused before it has finished sending, and the unread remainder is
    // discarded by \`node:http\` rather than buffered by this program.
    const response = await handleAdmitted(async () => {
      const requestInit = { method: req.method, headers };
      if (!isRead) {
        requestInit.body = await readRequestBody(req);
      }
      return new Request(url.toString(), requestInit);
    }, url.pathname);

    if (response.status === 404 && isRead) {
      const plan = staticResponsePlan(url.pathname, req.headers.range, {
        ifNoneMatch: req.headers['if-none-match'],
        ifModifiedSince: req.headers['if-modified-since'],
        ifRange: req.headers['if-range'],
      });
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
    const cacheControl = response.headers.get('cache-control');
    if (isCompressibleType(contentType, cacheControl)) {
      res.setHeader('vary', withVaryAcceptEncoding(res.getHeader('vary')));
    }
    const encoding = compressionFor(
      response.status,
      req.method,
      contentType,
      contentLengthOf(response.headers.get('content-length')),
      response.headers.get('content-encoding'),
      cacheControl,
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
      log('error', 'response stream failed', { error: String(error) });
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
        // The point of the limit is not to read the rest, so the rest is still
        // in flight on this socket when the answer goes out. Reusing it would
        // read those bytes as the beginning of the next request, which is why
        // a client that pools connections — every browser, and \`fetch\` itself —
        // saw a later, unrelated request die with ECONNRESET rather than
        // anything to do with the upload it made. Retiring the connection is
        // what RFC 9110 asks of a server that answers before the body is read;
        // the alternative, draining megabytes to keep it warm, is the cost the
        // limit exists to avoid.
        res.setHeader('connection', 'close');
      }
      res.end('Request body is too large');
      return;
    }
    log('error', 'request failed', {
      error: error instanceof Error ? error.message : String(error),
    });
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
server.keepAliveTimeout = positiveNumber('RUVYXA_KEEP_ALIVE_TIMEOUT', 65_000);
// Must exceed keepAliveTimeout, or Node can time out the headers of a request
// arriving on a connection it was still willing to keep.
server.headersTimeout = positiveNumber('RUVYXA_HEADERS_TIMEOUT', server.keepAliveTimeout + 5_000);

/**
 * Enforce that header deadline, because Node does not.
 *
 * \`headersTimeout\` is the documented knob for a request that never finishes
 * arriving, and on Node 24 it does not fire: a connection that writes
 * \`GET / HTTP/1.1\\r\\nHost: x\\r\\n\` and then stops is held open indefinitely, with
 * \`requestTimeout\` no better. Measured against a bare \`node:http\` server at
 * every value down to three seconds, so this is the runtime's behaviour rather
 * than anything about this program. Each such connection costs a socket and a
 * parser for as long as the client cares to hold it.
 *
 * Bun retires the same connection on its own, and Deno's server exposes no knob
 * for it — so this is the one transport with both the exposure and a way to
 * close it.
 *
 * Armed only while no request is being served, which is what keeps it from
 * being a request timeout: a slow upload, a long download, a streamed document
 * and a server-sent-event stream all clear it for as long as they run. The idle
 * keep-alive window is \`keepAliveTimeout\`'s, which does fire and is shorter, so
 * a well-behaved client never reaches this at all.
 */
const handshakeDeadlines = new WeakMap();

function clearHandshakeDeadline(socket) {
  const timer = handshakeDeadlines.get(socket);
  if (timer === undefined) return;
  clearTimeout(timer);
  handshakeDeadlines.delete(socket);
}

function armHandshakeDeadline(socket) {
  if (socket.destroyed) return;
  clearHandshakeDeadline(socket);
  const timer = setTimeout(() => socket.destroy(), server.headersTimeout);
  // Never a reason to keep the process alive: nothing is being served.
  if (typeof timer.unref === 'function') timer.unref();
  handshakeDeadlines.set(socket, timer);
}

server.on('connection', (socket) => {
  armHandshakeDeadline(socket);
  socket.on('close', () => clearHandshakeDeadline(socket));
});
server.on('request', (request, response) => {
  clearHandshakeDeadline(request.socket);
  // Re-armed once the answer is out, because the next request on a keep-alive
  // connection can stall halfway exactly like the first one.
  response.on('close', () => armHandshakeDeadline(request.socket));
});

let shuttingDown = false;

function shutdown(reason, exitCode) {
  // A second signal means now. An operator pressing Ctrl-C twice, or a platform
  // escalating, must not be held for a window that exists for a load balancer.
  if (shuttingDown) {
    log('warn', 'shutdown forced', { reason });
    process.exit(exitCode);
  }
  shuttingDown = true;
  log('info', 'draining connections', { reason, delay_ms: DRAIN_DELAY_MS });

  // A request that never finishes must not outlive the platform's own grace
  // period, or the process is SIGKILLed and the drain was pointless.
  const forceExit = setTimeout(() => {
    log('error', 'drain timed out', { detail: 'exiting with requests still open' });
    process.exit(exitCode);
  }, SHUTDOWN_GRACE_MS);
  forceExit.unref();

  // Still listening, and still answering, for this window: the readiness probe
  // reads 503 and stops routing here before the socket goes away.
  const stopAccepting = setTimeout(() => {
    // A request parked in the queue during a drain would wait for a slot this
    // process is about to stop handing out. Settling them refuses them instead,
    // which is an answer the caller can retry against the next instance.
    admission?.close();

    // Stop accepting new connections and wait for in-flight responses. Without
    // this a deploy kills the process outright and every request being served
    // at that moment fails in the user's browser.
    server.close(() => {
      clearTimeout(forceExit);
      log('info', 'shutdown complete');
      process.exit(exitCode);
    });

    // Idle keep-alive sockets hold the close callback for as long as
    // keepAliveTimeout, which would make every deploy wait a full minute for
    // connections carrying nothing. Requests in progress are unaffected.
    if (typeof server.closeIdleConnections === 'function') server.closeIdleConnections();
  }, DRAIN_DELAY_MS);
  stopAccepting.unref();
}

onShutdownSignal(shutdown);

// One route that rejects outside the request's own try/catch would otherwise
// terminate the process — Node's default for an unhandled rejection — and take
// every other in-flight request down with it. A single bad request must not be
// able to do that, so this is reported and the server keeps serving.
process.on('unhandledRejection', (reason) => {
  log('error', 'unhandled promise rejection', { reason: String(reason) });
});

// An uncaught exception is different: the process state after it is undefined,
// so continuing to serve from it is not trustworthy. Drain what is in flight,
// then leave with a non-zero code so the supervisor restarts a clean process.
process.on('uncaughtException', (error) => {
  log('error', 'uncaught exception', {
    error: error instanceof Error ? error.message : String(error),
  });
  shutdown('uncaught exception', 1);
});

server.listen(port, host, () => {
  log('info', 'listening on', {
    runtime: RUVYXA_RUNTIME,
    url: \`http://\${host === '0.0.0.0' ? 'localhost' : host}:\${port}\`,
  });
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
  // Neither runtime puts the peer on the `Request` — it arrives as the handler's
  // second argument, which is the whole reason this file never had one to weigh.
  const peerAddress =
    runtime === 'bun'
      ? `// Bun hands \`fetch\` its own \`Server\`, and the peer is asked of it by request.
function peerAddress(request, server) {
  return server?.requestIP?.(request)?.address ?? '';
}`
      : `// Deno's second argument carries the connection the request arrived on.
function peerAddress(_request, info) {
  return info?.remoteAddr?.hostname ?? '';
}`

  const openStaticBody =
    runtime === 'bun'
      ? `// \`Bun.file\` hands the socket a file rather than a copy of its bytes, and a
// slice of one is still a file. This is the path Bun optimizes, and the slice is
// handed over *as a file* rather than as its \`.stream()\`: measured against Bun
// 1.4.0, a sliced \`BunFile\` read through \`.text()\`, \`.bytes()\`, or as a
// response body all give the window, while the same slice's \`.stream()\` served
// by \`Bun.serve\` sends the whole file — a 206 whose body is the entire video,
// which is what a seek would have played.
//
// Bun leaves a handler's own \`content-range\` and status alone, but only while
// the handler answers the range. When it *declines* one — \`if-range\` naming a
// version this is no longer serving — \`Bun.serve\` applies its own range
// handling and turns the deliberate 200 into a 206 window of the *current*
// file. That is exactly the corrupt resumed download \`if-range\` exists to
// prevent, reintroduced one layer below the decision.
//
// Measured against Bun 1.4.0, a 200 carrying \`Range\`: a \`BunFile\` body is
// ranged to 206, and so is that file's own \`.stream()\` — Bun recognises its
// own file stream. A byte array is not ranged, but buffering the file is the
// peak-memory failure the streaming path exists to prevent. A stream Bun does
// not own is not ranged and still streams, so the file's stream is handed over
// through an identity transform. Only requests that actually declined a range
// pay for it; every other response keeps the sendfile path.
function openStaticBody(plan) {
  const file = Bun.file(plan.file);
  if (plan.declinedRange) return file.stream().pipeThrough(new TransformStream());
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
  // Outside \`handleRequest\`, and therefore outside the one place the security
  // defaults are added to a handler response. The Node transport's equivalent
  // is a \`catch\` inside a request whose \`res\` already carries them, so without
  // this the same build answered its own 500 with seven headers on one runtime
  // and none on another.
  error: (error) => {
    log('error', 'request failed', {
      error: error instanceof Error ? error.message : String(error),
    });
    return withSecurityHeaders(
      new Response('Internal Server Error', {
        status: 500,
        headers: { 'content-type': 'text/plain; charset=utf-8' },
      }),
    );
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
      log('info', 'listening on', {
        runtime: RUVYXA_RUNTIME,
        url: \`http://\${hostname === '0.0.0.0' ? 'localhost' : hostname}:\${boundPort}\`,
      });
    },
    // Outside \`handleRequest\`, for the reason the Bun transport's \`error\` hook
    // gives.
    onError: (error) => {
      log('error', 'request failed', {
        error: error instanceof Error ? error.message : String(error),
      });
      return withSecurityHeaders(
        new Response('Internal Server Error', {
          status: 500,
          headers: { 'content-type': 'text/plain; charset=utf-8' },
        }),
      );
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
      ? `log('info', 'listening on', {
  runtime: RUVYXA_RUNTIME,
  url: \`http://\${host === '0.0.0.0' ? 'localhost' : host}:\${port}\`,
});
`
      : ''

  return `${peerAddress}

${openStaticBody}

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
    // Added only where the plan is silent, which is what \`applySecurityHeaders\`
    // does on the Node transport and what \`apply_security_headers\` does on the
    // Axum host. Setting them over the plan agreed with both only because no
    // name collides today — a default that outranked a deliberate per-file
    // header would have been a divergence nobody could see until the day
    // something in \`staticResponsePlan\` set one.
    for (const [name, value] of Object.entries(DEFAULT_SECURITY_HEADERS)) {
      if (!headers.has(name)) headers.set(name, value);
    }
  }
  // A 416 carries no body, and a HEAD asks for the headers of the body it
  // would have received — including its \`content-length\`, which is why the
  // plan's headers are sent unchanged rather than recomputed for an empty one.
  if (plan.status === 416 || plan.status === 304 || method === 'HEAD') {
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
  const cacheControl = response.headers.get('cache-control');
  if (!isCompressibleType(contentType, cacheControl)) return response;

  const encoding = compressionFor(
    response.status,
    request.method,
    contentType,
    contentLengthOf(response.headers.get('content-length')),
    response.headers.get('content-encoding'),
    cacheControl,
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

/**
 * The request as the handler is allowed to see it.
 *
 * A forwarded identity from a peer that is neither loopback nor listed in
 * \`security.trustedProxyIps\` is the caller's own text, so it is deleted rather
 * than weighed — with it gone \`clientAddress\` falls through to \`unknown\`, which
 * buckets more aggressively than the traffic warrants. Rebuilt rather than
 * mutated: a \`Request\` these runtimes hand a fetch handler does not promise
 * mutable headers, and only \`init.headers\` is replaced, so the body is carried
 * across untouched and unread.
 */
function admissibleRequest(request, peer) {
  if (!FORWARDED_IDENTITY_HEADERS.some((name) => request.headers.has(name))) return request;
  if (peerMayStateClientIdentity(peer)) return request;
  const headers = new Headers(request.headers);
  for (const name of FORWARDED_IDENTITY_HEADERS) headers.delete(name);
  return new Request(request, { headers });
}

async function handleRequest(request, connection) {
  const url = new URL(request.url);
  const isRead = request.method === 'GET' || request.method === 'HEAD';

  // Ahead of everything, for the reason the Node transport gives — and through
  // \`withSecurityHeaders\` for the reason it does not have to: the Node
  // transport sets the defaults on \`res\` before it looks at the URL, so every
  // path it answers carries them, while here the only two places they were
  // added were the static and handler responses below. That left this
  // deployment's \`/__ruvyxa/health\` without a single one of them while the
  // identical build on Node, and the same path under \`ruvyxa start\`, carried
  // all seven.
  if (url.pathname === HEALTH_PATH) return withSecurityHeaders(healthResponse(request.method));
  if (url.pathname === METRICS_PATH) {
    const metrics = metricsResponse(request.method, request.headers.get('authorization') ?? '');
    if (metrics) return withSecurityHeaders(metrics);
  }

  // Hashed client bundles and asset-shaped paths are served before routing,
  // the order the Rust server uses. Page-shaped paths go through the handler
  // first so ISR revalidation and dynamic routes keep working; unmatched
  // paths fall back to static files.
  if (isRead && (url.pathname.startsWith('/__ruvyxa/') || isAssetPath(url.pathname))) {
    const plan = staticResponsePlan(url.pathname, request.headers.get('range'), {
      ifNoneMatch: request.headers.get('if-none-match'),
      ifModifiedSince: request.headers.get('if-modified-since'),
      ifRange: request.headers.get('if-range'),
    });
    if (plan) return withCompression(await staticResponse(plan, request.method), request);
  }

  // The request goes to the handler as it arrived, minus a forwarded identity
  // nothing in front of this server vouched for. Its own \`security.apiLimit\`
  // check reads \`content-length\` and answers 413 before a body is consumed,
  // so there is nothing for this transport to buffer or bound — the Node one
  // buffers only because \`node:http\` gave it a stream and not a \`Request\`.
  const response = await handleAdmitted(
    () => admissibleRequest(request, peerAddress(request, connection)),
    url.pathname,
  );

  if (response.status === 404 && isRead) {
    const plan = staticResponsePlan(url.pathname, request.headers.get('range'), {
      ifNoneMatch: request.headers.get('if-none-match'),
      ifModifiedSince: request.headers.get('if-modified-since'),
      ifRange: request.headers.get('if-range'),
    });
    if (plan) return withCompression(await staticResponse(plan, request.method), request);
  }
  return withCompression(withSecurityHeaders(response), request);
}

${listen}

let shuttingDown = false;

async function shutdown(reason, exitCode) {
  // A second signal means now. An operator pressing Ctrl-C twice, or a platform
  // escalating, must not be held for a window that exists for a load balancer.
  if (shuttingDown) {
    log('warn', 'shutdown forced', { reason });
    process.exit(exitCode);
  }
  shuttingDown = true;
  log('info', 'draining connections', { reason, delay_ms: DRAIN_DELAY_MS });

  // A request that never finishes must not outlive the platform's own grace
  // period, or the process is SIGKILLed and the drain was pointless.
  const forceExit = setTimeout(() => {
    log('error', 'drain timed out', { detail: 'exiting with requests still open' });
    process.exit(exitCode);
  }, SHUTDOWN_GRACE_MS);
  if (typeof forceExit?.unref === 'function') forceExit.unref();

  // Still listening, and still answering, for this window: the readiness probe
  // reads 503 and stops routing here before the socket goes away.
  if (DRAIN_DELAY_MS > 0) {
    await new Promise((resolve) => setTimeout(resolve, DRAIN_DELAY_MS));
  }
  // A request parked in the queue during a drain would wait for a slot this
  // process is about to stop handing out. Settling them refuses them instead,
  // which is an answer the caller can retry against the next instance.
  admission?.close();

  try {
    await closeServer();
  } catch (error) {
    log('error', 'shutdown failed', { error: String(error) });
  }
  clearTimeout(forceExit);
  log('info', 'shutdown complete');
  process.exit(exitCode);
}

onShutdownSignal(shutdown);

// One route that rejects outside the request's own try/catch would otherwise
// terminate the process and take every other in-flight request down with it. A
// single bad request must not be able to do that, so this is reported and the
// server keeps serving.
process.on('unhandledRejection', (reason) => {
  log('error', 'unhandled promise rejection', { reason: String(reason) });
});

// An uncaught exception is different: the process state after it is undefined,
// so continuing to serve from it is not trustworthy. Drain what is in flight,
// then leave with a non-zero code so the supervisor restarts a clean process.
process.on('uncaughtException', (error) => {
  log('error', 'uncaught exception', {
    error: error instanceof Error ? error.message : String(error),
  });
  void shutdown('uncaught exception', 1);
});

${banner}`
}
