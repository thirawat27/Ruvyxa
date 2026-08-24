/**
 * The deployment description `ruvyxa build` writes, and the rules for reading it.
 *
 * Ruvyxa's build output is provider-agnostic on purpose: one build, eleven
 * adapters, and no adapter is allowed to need a different build. What makes
 * that work is that every adapter answers the same two questions the same way —
 * *which URLs may be served from a file* and *what cache-control does each
 * class of emitted file carry* — and before `deploy-manifest.json` existed each
 * one answered them for itself by re-deriving the rules from route metadata.
 *
 * That is how `excludeStrategies: ['isr', 'ppr']` came to be hand-written in
 * six places: publishing an ISR page as a plain file makes the host answer it
 * from the build-time snapshot forever, the function that owns revalidation is
 * never invoked, and `revalidate` silently becomes decoration. An adapter that
 * forgets the rule produces a deployment that looks completely correct.
 *
 * The rules in this file are replayed against
 * `tests/fixtures/deploy-output-conformance.json`, the same table the Rust
 * writer (`crates/ruvyxa_cli/src/deploy_manifest.rs`) is tested against, so the
 * reader and the writer cannot drift.
 *
 * Nothing here touches the filesystem: the manifest arrives through
 * `BuildContext.deployManifest`, read once by the adapter runner, so this
 * module stays importable from an edge bundle.
 */

import { IMMUTABLE_CACHE_CONTROL, PUBLIC_ASSET_CACHE_CONTROL } from './utils.js'
import type { RenderStrategy } from './types.js'

/** The file a build writes its deployment description to, inside the build output. */
export const DEPLOY_MANIFEST_FILE = 'deploy-manifest.json'

/**
 * The contract version this package understands.
 *
 * A new field does not bump it — a reader that does not know a field ignores
 * it. It moves only when an existing field changes meaning or disappears, which
 * is exactly when an adapter must refuse the build rather than misread it.
 */
export const DEPLOY_MANIFEST_VERSION = 1

/** How a route is served once the build is deployed. */
export type DeployServeMode = 'static' | 'function'

/** One route, as the deployment sees it. */
export interface DeployRoute {
  id: string
  path: string
  kind: 'page' | 'api'
  /** Whether a CDN may answer this path from a file, or it must reach the server. */
  serve: DeployServeMode
  strategy: RenderStrategy
  runtime: 'node' | 'edge' | 'static'
  revalidate: number | null
  serverComponents: boolean
  /** Whether the served document carries a client bundle. */
  hydrate: boolean
  /** The pre-rendered document, relative to the prerender directory, or `null`. */
  document: string | null
  /** What the server sends with this route's document. */
  cacheControl: string
}

/** A path the build pre-rendered that no route entry names — one expansion of `getStaticParams`. */
export interface DeployPrerenderedPath {
  path: string
  document: string
  strategy: RenderStrategy
}

/** A cache-control rule for a class of emitted file. */
export interface DeployHeaderRule {
  /** URL pattern the rule applies to, as an anchored path prefix with a `(.*)` tail. */
  source: string
  class?: 'client' | 'asset'
  headers: Record<string, string>
}

/** The document a deployment answers an unmatched URL with. */
export interface DeployNotFound {
  status: number
  /** File name inside the prerender directory. */
  document: string
}

/** `deploy-manifest.json`, as written by `ruvyxa build`. */
export interface DeployManifest {
  version: number
  framework: 'ruvyxa'
  frameworkVersion: string
  /** Derived from the emitted output — stable across rebuilds of the same sources. */
  buildId: string
  basePath: string
  adapter: string | null
  directories: { client: string; assets: string; prerender: string; server: string }
  endpoints: Record<string, string>
  headers: DeployHeaderRule[]
  assetClasses: { client: string; asset: string; document: string }
  routes: DeployRoute[]
  staticPaths: string[]
  functionPaths: string[]
  prerendered: DeployPrerenderedPath[]
  notFound: DeployNotFound | null
  i18n: unknown
}

/** Every rendering strategy, in one place, so the derivations below are total. */
const RENDER_STRATEGIES: readonly RenderStrategy[] = ['ssr', 'ssg', 'isr', 'csr', 'ppr']

/**
 * Whether a route may be answered from a file, given what the build produced.
 *
 * The case that matters is the silent one: an ISR or PPR page *does* have a
 * pre-rendered document, and a host that checks the publish directory before
 * invoking the function will answer from it forever. Both are therefore served
 * by the function, which reads the same document as its first cache entry.
 */
export function routeServeMode(
  kind: 'page' | 'api',
  strategy: RenderStrategy,
  prerendered: boolean,
): DeployServeMode {
  if (kind === 'api') return 'function'
  if ((strategy === 'ssg' || strategy === 'csr') && prerendered) return 'static'
  return 'function'
}

/**
 * The cache-control a server sends with a document it just served.
 *
 * ISR advertises the project's own clock, so a CDN in front of the function can
 * hold the page for exactly as long as the project asked. A per-request render
 * advertises nothing cacheable: it may carry one visitor's data, and a shared
 * cache given no instruction has been observed to store it under heuristic
 * freshness.
 */
export function documentCacheControl(
  strategy: RenderStrategy,
  revalidate: number | null | undefined,
): string {
  if (strategy === 'isr') return `s-maxage=${revalidate ?? 60}, stale-while-revalidate`
  if (strategy === 'ssg' || strategy === 'csr') return DOCUMENT_CACHE_CONTROL
  return 'no-store'
}

/**
 * Cache-control for a pre-rendered document served as a file.
 *
 * Safe to store, never safe to pin: a redeploy replaces the document under the
 * same URL, and a reader holding a heuristically cached copy would keep seeing
 * the old site with nothing to tell it otherwise.
 */
export const DOCUMENT_CACHE_CONTROL = 'public, max-age=0, must-revalidate'

/**
 * Strategies whose pre-rendered documents must stay out of a publish directory.
 *
 * Derived from {@link routeServeMode} rather than listed, because a list is
 * only correct while somebody remembers to update it — and the six copies of
 * `['isr', 'ppr']` this replaces are what that looks like when nobody does.
 */
export function nonPublishableStrategies(): RenderStrategy[] {
  return RENDER_STRATEGIES.filter(
    (strategy) => routeServeMode('page', strategy, true) === 'function',
  )
}

/**
 * The cache-control rules a host should apply to the published directory.
 *
 * Taken from the manifest when the build wrote one, and otherwise derived from
 * the same constants the manifest is built from — an adapter must not fail
 * because it was handed an older build.
 */
export function deployHeaderRules(manifest?: DeployManifest | null): DeployHeaderRule[] {
  if (manifest?.headers?.length) return manifest.headers
  return [
    {
      source: '/__ruvyxa/client/(.*)',
      class: 'client',
      headers: { 'cache-control': IMMUTABLE_CACHE_CONTROL },
    },
    { source: '/(.*)', class: 'asset', headers: { 'cache-control': PUBLIC_ASSET_CACHE_CONTROL } },
  ]
}

/**
 * Read a deployment manifest out of a parsed JSON value, or `null`.
 *
 * A build from a newer Ruvyxa with a higher contract version is rejected rather
 * than partially understood: an adapter that guessed at fields it does not know
 * would produce a deployment nobody could debug. `null` means "derive it
 * yourself", which is what every adapter did before this file existed, so an
 * older build still deploys.
 */
export function parseDeployManifest(value: unknown): DeployManifest | null {
  if (!value || typeof value !== 'object') return null
  const manifest = value as Partial<DeployManifest>
  if (manifest.framework !== 'ruvyxa') return null
  if (typeof manifest.version !== 'number' || manifest.version > DEPLOY_MANIFEST_VERSION)
    return null
  if (!Array.isArray(manifest.routes)) return null
  return manifest as DeployManifest
}
