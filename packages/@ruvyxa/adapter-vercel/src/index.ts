import { createHash } from 'node:crypto'

import type { Adapter, AdapterOutput, BuildContext, DeployRoute } from '@ruvyxa/core'
import {
  DEFAULT_IMAGE_MAX_WIDTH,
  DEFAULT_SECURITY_HEADERS,
  clientBuildOutput,
  nonPublishableStrategies,
  runtimeBuildPolicy,
  staticAssetPattern,
  validateBuildContext,
} from '@ruvyxa/core'
import { createCanonicalRouteMatcher, normalizeMatchPath } from '@ruvyxa/core/route-match'

/**
 * Options for Vercel deployment.
 */
export interface VercelAdapterOptions {
  /**
   * Emit a Web-standard Edge Function instead of a Node.js serverless
   * function. Edge mode supports SSR, SSG, CSR, and API routes; ISR/PPR need
   * the writable Node cache and are rejected during build capability checks.
   * @default false
   */
  edge?: boolean
  /**
   * Put the routes that declared `export const runtime = 'edge'` in their own
   * Edge Function, beside the Node one that answers everything else.
   *
   * Off by default, and deliberately so: one function is simpler, and a route
   * on Node is always correct — every API an edge route may use exists there
   * too. Turn this on when a route's latency genuinely wants a point of
   * presence rather than a region.
   *
   * A route is eligible when it needs nothing the framework keeps in one place.
   * Server components and server actions are answered through
   * `/__ruvyxa/rsc`, `/__ruvyxa/flight`, and `/__ruvyxa/action` — single paths
   * owned by one function — and ISR and PPR need a writable store this edge
   * function has none of. A route that declares `edge` and needs any of them is
   * refused by name rather than silently left on Node, because being quietly
   * ignored is how a latency decision stops being true without anyone noticing.
   * @default false
   */
  splitEdgeRoutes?: boolean
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
   * @default 'nodejs24.x'
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
function vercelHandlerSource(
  runtimePolicy: unknown,
  imageSizes: readonly number[],
  bypassToken: string,
): string {
  return `import { createHandler, prerenderRelativePath } from './serverless-handler.mjs';
import { applyPluginHttp, loadActionModule, loadRouteModule } from './route-modules.mjs';
// Imported, not read from disk: a platform that re-bundles the function only
// carries files it can resolve statically (see the netlify adapter, where a
// readFileSync of a sibling manifest.json crashed the deployed function).
import manifest from './manifest.mjs';
import { AsyncLocalStorage } from 'node:async_hooks';
import { readFileSync, writeFileSync, mkdirSync, statSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';

const runtimePolicy = ${JSON.stringify(runtimePolicy ?? {})};
${vercelImageSizesSource(imageSizes)}
const prerenderDir = path.join(import.meta.dirname, 'prerender');
// The function bundle directory is read-only at runtime; only the platform
// tmp directory accepts writes. ISR revalidations land there and are read
// back before the bundled deploy-time prerender output.
const isrCacheDir = path.join(os.tmpdir(), 'ruvyxa-isr-cache');

// The Prerender Function's cache lives in front of this function, so writing a
// fresh document to the store below does not change what a visitor is served
// until the window expires. Vercel documents one way to invalidate it: a GET to
// the path carrying \`x-prerender-revalidate: <bypassToken>\`. That is what makes
// \`revalidatePath()\` mean the same thing here as it does under \`ruvyxa start\`.
const BYPASS_TOKEN = ${JSON.stringify(bypassToken)};
// The origin of the request being served, read by the purge below: a
// request-scoped value rather than a configured site URL, so a preview
// deployment purges its own domain and a custom domain purges its own.
//
// Held in an AsyncLocalStorage and not in a module-level variable. One instance
// of this function answers more than one request at a time, so a plain
// assignment is overwritten by whatever arrived next — and the purge then went
// to the other request's domain, leaving the page it was asked to drop cached
// on the domain a visitor was actually reading.
const requestOrigin = new AsyncLocalStorage();

async function revalidateOnVercel(pathname) {
  const origin = requestOrigin.getStore();
  if (origin === undefined) return;
  try {
    // This re-enters the function, which renders and stores the page again —
    // an ordinary refresh, not a forced one, so it cannot schedule a third
    // request and loop.
    await fetch(new URL(pathname, origin), {
      method: 'GET',
      headers: { 'x-prerender-revalidate': BYPASS_TOKEN },
    });
  } catch (error) {
    console.error('[ruvyxa] could not revalidate ' + pathname + ' on the CDN:', error);
  }
}

const readEntry = (htmlPath, revalidate) => {
  const html = readFileSync(htmlPath, 'utf8');
  const stale = Date.now() - statSync(htmlPath).mtimeMs >= revalidate * 1000;
  return { html, stale };
};

const handler = createHandler({
  routes: manifest.routes,
  middleware: runtimePolicy.middleware,
  i18n: manifest.i18n,
  optimizeImage: runtimePolicy.image?.onDemand === true ? optimizeImage : undefined,
  imageQuality: runtimePolicy.image?.quality,
  importPage: loadRouteModule,
  importApi: loadRouteModule,
  importAction: loadActionModule,
  pluginHttp: applyPluginHttp,
  security: runtimePolicy.security,
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
  writePrerendered: (pathname, html, revalidate, forced) => {
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return;
    const htmlPath = path.join(isrCacheDir, relative);
    mkdirSync(path.dirname(htmlPath), { recursive: true });
    writeFileSync(htmlPath, html, 'utf8');
    if (forced === true) return revalidateOnVercel(pathname);
  },
  // The project's own not-found page, pre-rendered by the build and carried
  // inline in the manifest: an unmatched URL is answered with the page the
  // application actually wrote, on every host.
  notFoundDocument: manifest.notFoundDocument,
  supportedStrategies: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
  // Vercel writes \`X-Vercel-Forwarded-For\` itself, and overwrites
  // \`X-Forwarded-For\` rather than forwarding an external one — its own docs
  // say that is to prevent IP spoofing — so on this platform the header is the
  // client. Declared here rather than assumed by the handler: the same handler
  // runs on a self-hosted server where nothing writes it.
  clientIpHeaders: ['x-vercel-forwarded-for'],
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
  const url = new URL(req.url, \`https://\${req.headers.host || 'localhost'}\`);
  return requestOrigin.run(url.origin, () => serve(req, res, context, url));
}

async function serve(req, res, context, url) {
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
  if (!response.body) {
    res.end();
    return;
  }
  // Forwarded as it arrives rather than collected first. Buffering held the
  // whole response in the function's memory and delayed the first byte until
  // the last one was produced, which is exactly wrong for a streamed PPR shell
  // or a large API response. The supportsResponseStreaming flag in
  // .vc-config.json is the half of this that tells Vercel to pass the bytes
  // straight through.
  await pipeline(Readable.fromWeb(response.body), res);
}
`
}

/** Vercel Edge entry point: Request -> Response with no Node.js imports. */
function vercelEdgeHandlerSource(runtimePolicy: unknown, imageSizes: readonly number[]): string {
  return `import { createHandler } from './serverless-handler.mjs';
import { applyPluginHttp, loadActionModule, loadRouteModule } from './route-modules.mjs';
import manifest from './manifest.mjs';

const runtimePolicy = ${JSON.stringify(runtimePolicy ?? {})};
${vercelImageSizesSource(imageSizes)}
const handler = createHandler({
  routes: manifest.routes,
  importPage: loadRouteModule,
  importApi: loadRouteModule,
  importAction: loadActionModule,
  pluginHttp: applyPluginHttp,
  security: runtimePolicy.security,
  middleware: runtimePolicy.middleware,
  i18n: manifest.i18n,
  optimizeImage: runtimePolicy.image?.onDemand === true ? optimizeImage : undefined,
  imageQuality: runtimePolicy.image?.quality,
  // The project's own not-found page, pre-rendered by the build and carried
  // inline in the manifest: an unmatched URL is answered with the page the
  // application actually wrote, on every host.
  notFoundDocument: manifest.notFoundDocument,
  supportedStrategies: ['ssr', 'ssg', 'csr', 'api'],
  // Vercel writes \`X-Vercel-Forwarded-For\` itself, and overwrites
  // \`X-Forwarded-For\` rather than forwarding an external one — its own docs
  // say that is to prevent IP spoofing — so on this platform the header is the
  // client. Declared here rather than assumed by the handler: the same handler
  // runs on a self-hosted server where nothing writes it.
  clientIpHeaders: ['x-vercel-forwarded-for'],
});

export default function(request, context) {
  return handler(request, context);
}
`
}

/**
 * The widths this deployment declares to Vercel, ascending.
 *
 * Empty when the project did not turn on-demand optimization on, which is also
 * what makes `images` absent from `config.json` and `/_vercel/image` a 404.
 */
function vercelImageSizes(runtimePolicy: Readonly<Record<string, unknown>>): number[] {
  const image = runtimePolicy.image
  if (!image || typeof image !== 'object' || Array.isArray(image)) return []
  const policy = image as Readonly<Record<string, unknown>>
  if (policy.onDemand !== true) return []
  const maxWidth = typeof policy.maxWidth === 'number' ? policy.maxWidth : DEFAULT_IMAGE_MAX_WIDTH
  const configured = Array.isArray(policy.sizes) ? policy.sizes : []
  const sizes = configured
    .filter((size): size is number => Number.isInteger(size) && size >= 16 && size <= maxWidth)
    .filter((size, index, values) => values.indexOf(size) === index)
    .sort((left, right) => left - right)
  if (sizes.length === 0) sizes.push(640, 750, 828, 1080, 1200, 1920, 2048, 3840)
  const bounded = sizes.filter((size) => size <= maxWidth)
  // `sizes` is required and every accepted `w` has to appear in it, so an empty
  // array would declare an optimizer that rejects every request it is given. A
  // `maxWidth` below the smallest default lands here, and the width it names is
  // the one width that is certainly allowed.
  return bounded.length > 0 ? bounded : [maxWidth]
}

function vercelImagesConfig(runtimePolicy: Readonly<Record<string, unknown>>): object | undefined {
  const sizes = vercelImageSizes(runtimePolicy)
  if (sizes.length === 0) return undefined
  return {
    sizes,
    domains: [],
    minimumCacheTTL: 86400,
    formats: ['image/avif', 'image/webp'],
    localPatterns: [{ pathname: '^/(?!__ruvyxa/).*$' }],
  }
}

/**
 * The `optimizeImage` both entry points carry, written once.
 *
 * Vercel answers `400` for any `?w=` its `images.sizes` did not declare, and
 * `<Image>` puts the author's own `width` into the `srcset` without snapping it
 * — so forwarding the requested width verbatim broke exactly the images that
 * render correctly under `ruvyxa start`. The request is widened to the nearest
 * declared size instead, which is the size Vercel would have served anyway.
 */
function vercelImageSizesSource(imageSizes: readonly number[]): string {
  return `const imageSizes = ${JSON.stringify(imageSizes)};

function optimizeImage(request, { src, width, quality }) {
  if (width > (runtimePolicy.image?.maxWidth ?? ${DEFAULT_IMAGE_MAX_WIDTH})) {
    return new Response('Image width exceeds configured maximum', { status: 400 });
  }
  const allowedWidth =
    imageSizes.find((size) => size >= width) ?? imageSizes[imageSizes.length - 1] ?? width;
  const destination = new URL('/_vercel/image', request.url);
  destination.searchParams.set('url', src);
  destination.searchParams.set('w', String(allowedWidth));
  destination.searchParams.set('q', String(quality));
  return Response.redirect(destination, 307);
}
`
}

/** Every emitted config file ends with one, the way the rest of this adapter writes them. */
const NEWLINE = '\n'

/**
 * Where a route path's function directory lives, without the `.func` suffix.
 *
 * `/` is `index`, matching the static file of the same name — the Build Output
 * API resolves a function exactly the way it resolves a file.
 */
function functionOutputName(routePath: string): string {
  const trimmed = routePath.replace(/^\/+/, '').replace(/\/+$/, '')
  return trimmed === '' ? 'index' : trimmed
}

/**
 * The token that turns a Prerender Function's cache off for one request.
 *
 * It gates two documented features at once: setting `__prerender_bypass` to it
 * enables Draft Mode, and a `GET` carrying `x-prerender-revalidate: <token>`
 * revalidates that path on demand. Both are how Vercel's own ISR works, so
 * without a token `revalidatePath()` can refresh the function's store and leave
 * the CDN serving the old document until the window expires.
 *
 * `RUVYXA_PREVIEW_SECRET` wins when the project sets one, which is the only way
 * to hold a value the build output does not contain. Otherwise it is derived
 * from the build id — which is itself derived from the emitted output — so two
 * builds of one commit still produce identical bytes and `verify:reproducible`
 * keeps meaning something. A random token per build would not.
 *
 * That default is a convenience and not a secret, and the difference is worth
 * naming rather than leaving in a hash: the build id is a digest of the build's
 * own public surface — asset names, document paths, the route table — so anyone
 * willing to reconstruct that input reconstructs the token. What the token buys
 * them is a cache bypass, which turns every request into a render. A project
 * that cares sets `RUVYXA_PREVIEW_SECRET`; `docs/en/20-platform-adapter-guide.md`
 * says so under "On-demand revalidation on Vercel", because a mitigation nobody
 * can find is not one.
 */
function prerenderBypassToken(buildId: string): string {
  const configured = process.env.RUVYXA_PREVIEW_SECRET
  if (typeof configured === 'string' && configured.trim() !== '') {
    return createHash('sha256').update(configured).digest('hex').slice(0, 32)
  }
  return createHash('sha256')
    .update(`ruvyxa-prerender-bypass:${buildId}`)
    .digest('hex')
    .slice(0, 32)
}

/**
 * The routes that become Prerender Functions, and the paths they answer.
 *
 * ISR only. PPR streams its dynamic holes at request time, and a cache in front
 * of a streamed shell is a different mechanism with different failure modes —
 * it stays on the catch-all until it is built deliberately.
 *
 * A path with a dynamic segment is left out too: a Prerender Function is mounted
 * at a literal path, so `/blog/[slug]` would cache one entry for every slug
 * under the pattern's own name. The expansions a build did produce are here by
 * name, through the manifest's `prerendered` list.
 */
function prerenderPaths(ctx: BuildContext): Array<{ path: string; revalidate: number }> {
  const manifest = ctx.deployManifest
  if (!manifest) return []
  const seen = new Set<string>()
  const paths: Array<{ path: string; revalidate: number }> = []
  const add = (routePath: string, revalidate: number | null | undefined) => {
    if (routePath.includes('[') || seen.has(routePath)) return
    seen.add(routePath)
    // Vercel measures the window in whole seconds and treats `0` as "no
    // expiration named"; the project's `revalidate: 0` means the opposite, so
    // it becomes the shortest window the platform accepts.
    paths.push({ path: routePath, revalidate: Math.max(1, revalidate ?? 60) })
  }
  // Only an ISR page can have produced an expansion, so only those are
  // candidates. A dynamic API route or a PPR page can match the same path and
  // never rendered it.
  const isrPages = (manifest.routes ?? []).filter(
    (route) => route.kind === 'page' && route.strategy === 'isr',
  )
  for (const route of isrPages) {
    add(route.path, route.revalidate)
  }
  // The router's own matcher, compiled once for the whole list: it applies
  // static-before-dynamic-before-catch-all precedence, which raw manifest order
  // does not. Two patterns can fill one path — `[...slug]` and `[[...slug]]`
  // both do — so "the first pattern that fits" answers by file order, not by
  // which route the visitor's request would actually reach.
  const parentOf = createCanonicalRouteMatcher(isrPages)
  for (const entry of manifest.prerendered ?? []) {
    if (entry.strategy !== 'isr') continue
    // An expansion has no revalidate of its own; it inherits its route's. A
    // path no ISR page claims takes the default rather than a longer window
    // borrowed from a route that did not render it.
    add(entry.path, parentOf(normalizeMatchPath(entry.path))?.route.revalidate)
  }
  return paths
}

/** The Build Output `src` pattern that matches one route path. */
function routeSourcePattern(routePath: string): string {
  const body = routePath
    .split('/')
    .filter(Boolean)
    .map((segment) => {
      // An optional catch-all also matches the parent path itself, so it is the
      // one shape whose slash is part of the group rather than before it.
      if (segment.startsWith('[[...') && segment.endsWith(']]')) return '(?:/.*)?'
      if (segment.startsWith('[...') && segment.endsWith(']')) return '/.+'
      if (segment.startsWith('[') && segment.endsWith(']')) return '/[^/]+'
      return `/${segment.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`
    })
    .join('')
  return `^${body === '' ? '/' : body}$`
}

/** The routes an Edge Function may answer, and why each of the rest may not. */
function partitionEdgeRoutes(ctx: BuildContext): { edge: DeployRoute[]; refused: string[] } {
  const routes = ctx.deployManifest?.routes ?? []
  const edge: DeployRoute[] = []
  const refused: string[] = []
  for (const route of routes) {
    if (route.runtime !== 'edge') continue
    // A route served from a file never reaches a function at all, so which
    // function would have answered it is not a question.
    if (route.serve !== 'function') continue
    const reasons: string[] = []
    if (route.serverComponents) reasons.push('renders server components')
    if (route.strategy === 'isr' || route.strategy === 'ppr') {
      reasons.push(`uses ${route.strategy}, which needs a writable document store`)
    }
    if (reasons.length > 0) {
      refused.push(`${route.path} (${reasons.join('; ')})`)
      continue
    }
    edge.push(route)
  }
  return { edge, refused }
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
 * import { vercel } from "@ruvyxa/adapter-vercel"
 *
 * export default config({
 *   adapter: vercel()
 * })
 * ```
 */
export function vercel(options: VercelAdapterOptions = {}): Adapter {
  if (options.functionsDir !== undefined && typeof options.functionsDir !== 'string') {
    throw new Error(
      `[RUV2001] vercelAdapter: "functionsDir" must be a string, got ${typeof options.functionsDir}`,
    )
  }

  if (options.functionsDir !== undefined && options.functionsDir.trim() === '') {
    throw new Error(`[RUV2001] vercelAdapter: "functionsDir" must not be an empty string`)
  }

  if (options.edge === true && options.runtime !== undefined) {
    throw new Error(`[RUV2001] vercelAdapter: "runtime" cannot be set when "edge" is true`)
  }
  if (options.edge === true && options.maxDuration !== undefined) {
    throw new Error(`[RUV2001] vercelAdapter: "maxDuration" cannot be set when "edge" is true`)
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
    target: options.edge === true ? 'edge' : 'serverless',
    supports:
      options.edge === true
        ? ['ssr', 'ssg', 'csr', 'api']
        : ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'vercelAdapter')
      const functionsDir = options.functionsDir ?? `${ctx.outDir}/functions`
      const runtime = options.runtime ?? 'nodejs24.x'
      const maxDuration = options.maxDuration ?? 10
      const edge = options.edge === true
      const runtimePolicy = runtimeBuildPolicy(ctx)
      const images = vercelImagesConfig(runtimePolicy)
      const imageSizes = vercelImageSizes(runtimePolicy)
      const STATIC_ASSET_PATTERN = staticAssetPattern()
      const isrPaths = prerenderPaths(ctx)
      const bypassToken = prerenderBypassToken(ctx.deployManifest?.buildId ?? '')

      // Routes that asked for a point of presence, and got one.
      //
      // Only when the project turned it on, and never in whole-app edge mode,
      // where there is one function and it is already the edge one.
      const split = options.splitEdgeRoutes === true && !edge
      const partition = split ? partitionEdgeRoutes(ctx) : { edge: [], refused: [] }
      if (partition.refused.length > 0) {
        throw new Error(
          `[RUV2203] vercelAdapter: these routes declare \`runtime = 'edge'\` but cannot be ` +
            `answered by an Edge Function — ${partition.refused.join(', ')}. Remove the ` +
            `declaration to keep them on Node, or drop \`splitEdgeRoutes\`.`,
        )
      }
      const edgeRoutes = partition.edge
      const edgeFunctionPath = '__ruvyxa_edge'
      // Ahead of the catch-all, so an edge path reaches its own function; every
      // other path falls through to the Node one exactly as before.
      const edgeConfigRoutes = edgeRoutes.map((route) => ({
        src: routeSourcePattern(route.path),
        dest: `/${edgeFunctionPath}`,
      }))

      // The Node function keeps every route, including the ones the edge
      // function answers. That is deliberate rather than an oversight: it is
      // the catch-all, and `routeSourcePattern` is a pattern rather than the
      // router — `/edge/` with a trailing slash does not match `^/edge$`, and a
      // rewrite can land on a path the pattern never saw. Carrying the route in
      // both means such a request renders the right page from the wrong
      // runtime, instead of 404ing from the right one. The cost is that the
      // route's modules are compiled twice.

      /**
       * The Prerender Functions, which is how Vercel itself does ISR.
       *
       * Each one is the *same* function mounted at a second path, because the
       * `<name>.prerender-config.json` that carries the window has to sit beside
       * a `<name>.func`. Vercel then caches that path's response at the edge for
       * `expiration` seconds and re-invokes the function behind the scenes —
       * rather than the function answering every request and keeping its own
       * copy in a per-instance tmp directory.
       *
       * No `fallback` file is emitted: the deploy-time document is already
       * inside the function bundle, which `readPrerendered` answers the first
       * request from, so a second copy beside the config would only be a second
       * thing to keep in step.
       */
      const prerenderArtifacts = (
        base: string,
        scope?: 'project',
      ): NonNullable<AdapterOutput['artifacts']> =>
        edge
          ? []
          : isrPaths.flatMap(({ path: routePath, revalidate }) => {
              const name = functionOutputName(routePath)
              return [
                {
                  kind: 'function-alias' as const,
                  path: `${base}/functions/${name}.func`,
                  aliasOf: `${base}/functions/__ruvyxa_handler.func`,
                  ...(scope ? { scope } : {}),
                },
                {
                  kind: 'file' as const,
                  path: `${base}/functions/${name}.prerender-config.json`,
                  ...(scope ? { scope } : {}),
                  contents:
                    JSON.stringify(
                      {
                        expiration: revalidate,
                        // One entry per path. Left undefined every distinct
                        // query string would cache separately, so a page linked
                        // with `?utm_source=` would be regenerated per campaign.
                        allowQuery: [],
                        bypassToken,
                      },
                      null,
                      2,
                    ) + NEWLINE,
                },
              ]
            })

      /** The second function, when there is one. `base` is the scope's prefix. */
      const edgeFunctionArtifacts = (
        base: string,
        scope?: 'project',
      ): NonNullable<AdapterOutput['artifacts']> =>
        edgeRoutes.length === 0
          ? []
          : [
              {
                kind: 'function',
                path: `${base}/functions/${edgeFunctionPath}.func`,
                ...(scope ? { scope } : {}),
                handlerSource: vercelEdgeHandlerSource(runtimePolicy, imageSizes),
                // Its own runtime and its own slice of the routes: the registry
                // this function compiles resolves `worker`/`edge-light`
                // conditions, and the manifest it routes with names only these
                // routes, so it can never be asked for one it does not carry.
                target: 'edge',
                routes: edgeRoutes.map((route) => route.id),
              },
              {
                kind: 'file',
                path: `${base}/functions/${edgeFunctionPath}.func/.vc-config.json`,
                ...(scope ? { scope } : {}),
                contents:
                  JSON.stringify(
                    {
                      runtime: 'edge',
                      entrypoint: 'index.mjs',
                      ...(options.regions === undefined ? {} : { regions: options.regions }),
                    },
                    null,
                    2,
                  ) + '\n',
              },
            ]

      // Build Output API v3 config with dynamic routing
      const buildOutputConfig = JSON.stringify(
        {
          version: 3,
          ...(images === undefined ? {} : { images }),
          routes: [
            {
              // The security defaults, on everything — including the responses
              // the function never sees.
              //
              // `handle: filesystem` below answers a pre-rendered document and
              // every public file from Vercel's own edge, so `createHandler`,
              // which is where these headers are set, is never invoked for
              // them. A page that is framed-denied under `ruvyxa start` was
              // framable the moment it was pre-rendered and deployed, and every
              // other check stayed green: the markup was right and the status
              // was 200. `continue: true` attaches the headers and lets routing
              // carry on, so this changes what is served and not where from.
              src: '/(.*)',
              headers: DEFAULT_SECURITY_HEADERS,
              continue: true,
            },
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
            // Edge-declared routes first, then everything else.
            ...edgeConfigRoutes,
            // All unmatched requests go to the serverless function
            { src: '/(.*)', dest: '/__ruvyxa_handler' },
          ],
        },
        null,
        2,
      )

      // Vercel function configuration
      const vcConfig = JSON.stringify(
        edge
          ? {
              runtime: 'edge',
              entrypoint: 'index.mjs',
              ...(options.regions === undefined ? {} : { regions: options.regions }),
            }
          : {
              runtime,
              handler: 'index.mjs',
              maxDuration,
              launcherType: 'Nodejs',
              // The handler pipes the response body through; without this the
              // platform waits for the function to finish before sending any
              // of it.
              supportsResponseStreaming: true,
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
                excludeStrategies: nonPublishableStrategies(),
              },
              {
                kind: 'function',
                path: '.vercel/output/functions/__ruvyxa_handler.func',
                scope: 'project',
                handlerSource: edge
                  ? vercelEdgeHandlerSource(runtimePolicy, imageSizes)
                  : vercelHandlerSource(runtimePolicy, imageSizes, bypassToken),
              },
              {
                kind: 'file',
                path: '.vercel/output/functions/__ruvyxa_handler.func/.vc-config.json',
                scope: 'project',
                contents: vcConfig + '\n',
              },
              ...prerenderArtifacts('.vercel/output', 'project'),
              ...edgeFunctionArtifacts('.vercel/output', 'project'),
              {
                kind: 'file',
                path: '.vercel/output/config.json',
                scope: 'project',
                contents: buildOutputConfig + '\n',
              },
            ]

      return {
        name: 'vercel',
        target: edge ? 'edge' : 'serverless',
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
            excludeStrategies: nonPublishableStrategies(),
          },
          // Serverless function bundle
          {
            kind: 'function',
            path: 'deploy/vercel/.vercel/output/functions/__ruvyxa_handler.func',
            handlerSource: edge
              ? vercelEdgeHandlerSource(runtimePolicy, imageSizes)
              : vercelHandlerSource(runtimePolicy, imageSizes, bypassToken),
          },
          // Function config
          {
            kind: 'file',
            path: 'deploy/vercel/.vercel/output/functions/__ruvyxa_handler.func/.vc-config.json',
            contents: vcConfig + '\n',
          },
          ...prerenderArtifacts('deploy/vercel/.vercel/output'),
          ...edgeFunctionArtifacts('deploy/vercel/.vercel/output'),
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

export default vercel
