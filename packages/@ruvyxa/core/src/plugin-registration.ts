/**
 * The rules a plugin registration has to satisfy, stated once.
 *
 * Two modules used to state them: `plugin-harness.ts`, which validates a plugin
 * in tests and in `@ruvyxa/core`'s own harness, and
 * `packages/ruvyxa/runtime/plugin-http.mjs`, which validates the same plugin at
 * request time in every host. Each carried a comment saying it mirrored the
 * other, which is the arrangement `AGENTS.md` names as the thing that drifts —
 * and it had already drifted: the harness accepted an `http.route()` on a
 * reserved framework path that the runtime refuses, so a plugin could pass its
 * own tests and be rejected by the server that runs it.
 *
 * This module is copied into `packages/ruvyxa/runtime/plugin-registration.mjs`
 * by `pnpm --filter ruvyxa sync:runtime`, the same way `route-match` and
 * `origin-policy` are, because a serverless function bundle resolves no bare
 * specifiers. It therefore imports nothing — not even a Node builtin.
 */

/**
 * Paths the framework answers itself, which a plugin may not claim.
 *
 * Held to `tests/fixtures/framework-endpoint-conformance.json` together with
 * `RESERVED_FRAMEWORK_ROUTES` in the native server, which panics inside axum if
 * a second handler registers one of these paths.
 */
export const RESERVED_FRAMEWORK_PATHS = Object.freeze([
  '/__ruvyxa/hmr',
  '/__ruvyxa/client',
  '/__ruvyxa/action',
  '/__ruvyxa/flight',
  '/__ruvyxa/rsc',
  '/__ruvyxa/trace',
  '/__ruvyxa/devtools',
  '/__ruvyxa/devtools/data',
  '/__ruvyxa/image',
  '/__ruvyxa/hydration-loader.js',
  '/__ruvyxa/client/route-manifest.json',
  '/__ruvyxa/client/vendor',
  '/__ruvyxa/health',
])

/**
 * Whether a value is an exact application path for `http.route()`.
 *
 * Compared by string equality in the plugin stage and never handed to a router,
 * so the router's own syntax is not this rule's question — which is why it is
 * looser than {@link isLiteralTransportPath} below and deliberately stays so.
 */
export function isExactApplicationPath(value: string): boolean {
  return (
    value.startsWith('/') && !value.includes('?') && !value.includes('#') && !value.includes('*')
  )
}

/**
 * Whether a `realtime@1` / `presence@1` transport path is a literal route.
 *
 * A socket transport is not an `http.route`: the Axum host registers this
 * string on its router, so any character that router assigns a meaning to is a
 * wildcard rather than a path. The check used to be a denylist of `?`, `#` and
 * `*` — the axum 0.7 wildcard set — in both hosts, long after the workspace
 * moved to axum 0.8, where a capture is `{name}` and a catch-all `{*rest}`.
 * `/{room}` passed both guards and registered a single-segment wildcard that
 * shadowed every one-segment project page; `/{` passed both and panicked
 * `matchit` inside `Router::route`, which is the outcome the guards exist to
 * prevent.
 *
 * A denylist is only correct while it tracks the router's syntax and nothing
 * makes it, so the rule is an allowlist: one or more `/`-prefixed segments of
 * RFC 3986 unreserved characters, which is a literal path in every router
 * version and can never acquire a meaning.
 *
 * The twin of `is_literal_transport_path` in
 * `crates/ruvyxa_dev_server/src/lib.rs`, held level by `transportPaths` in
 * `tests/fixtures/framework-endpoint-conformance.json`.
 */
export function isLiteralTransportPath(value: unknown): boolean {
  return typeof value === 'string' && /^(\/[A-Za-z0-9._~-]+)+$/.test(value)
}

/** What a refused transport path is told it may contain. */
export const TRANSPORT_PATH_RULE =
  'must be an exact absolute path of `/`-prefixed segments containing only letters, digits, `-`, `.`, `_`, or `~`'

/** Whether a path is one the framework answers and a plugin may not take. */
export function isReservedFrameworkPath(value: string): boolean {
  return RESERVED_FRAMEWORK_PATHS.includes(value)
}

/** A validated `realtime@1` claim. */
export interface NormalizedRealtime {
  readonly id: 'realtime@1'
  readonly plugin: string
  readonly path: string
  readonly heartbeatMs: number
  readonly capacity: number
}

/** A validated `presence@1` claim. */
export interface NormalizedPresence {
  readonly id: 'presence@1'
  readonly plugin: string
  readonly path: string
  readonly heartbeatMs: number
}

/**
 * The bounds a heartbeat has to fall inside, shared by both transports.
 *
 * Below five seconds a heartbeat is indistinguishable from traffic and costs
 * more than the connection it is checking; above two minutes an intermediary
 * has usually dropped the socket before the next one arrives.
 */
const HEARTBEAT_MIN_MS = 5_000
const HEARTBEAT_MAX_MS = 120_000

/** Validate a `realtime@1` claim, or throw naming the plugin and the rule. */
export function normalizeRealtime(plugin: string, value: unknown): NormalizedRealtime {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`plugin "${plugin}" native.claim('realtime@1') expects an options object`)
  }
  const options = value as { path?: unknown; heartbeatMs?: unknown; capacity?: unknown }
  const pathValue = (options.path ?? '/__ruvyxa/realtime') as string
  const heartbeatMs = (options.heartbeatMs ?? 25_000) as number
  const capacity = (options.capacity ?? 256) as number
  if (!isLiteralTransportPath(pathValue)) {
    throw new TypeError(`plugin "${plugin}" realtime path ${TRANSPORT_PATH_RULE}`)
  }
  if (
    !Number.isInteger(heartbeatMs) ||
    heartbeatMs < HEARTBEAT_MIN_MS ||
    heartbeatMs > HEARTBEAT_MAX_MS
  ) {
    throw new TypeError(`plugin "${plugin}" realtime heartbeatMs must be between 5000 and 120000`)
  }
  if (!Number.isInteger(capacity) || capacity < 16 || capacity > 4096) {
    throw new TypeError(`plugin "${plugin}" realtime capacity must be between 16 and 4096`)
  }
  if (isReservedFrameworkPath(pathValue)) {
    throw new TypeError(
      `plugin "${plugin}" realtime path "${pathValue}" collides with a reserved framework route`,
    )
  }
  return Object.freeze({ id: 'realtime@1', plugin, path: pathValue, heartbeatMs, capacity })
}

/** Validate a `presence@1` claim, or throw naming the plugin and the rule. */
export function normalizePresence(plugin: string, value: unknown): NormalizedPresence {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`plugin "${plugin}" native.claim('presence@1') expects an options object`)
  }
  const options = value as { path?: unknown; heartbeatMs?: unknown }
  const pathValue = (options.path ?? '/__ruvyxa/collab') as string
  const heartbeatMs = (options.heartbeatMs ?? 25_000) as number
  if (!isLiteralTransportPath(pathValue)) {
    throw new TypeError(`plugin "${plugin}" presence path ${TRANSPORT_PATH_RULE}`)
  }
  if (
    !Number.isInteger(heartbeatMs) ||
    heartbeatMs < HEARTBEAT_MIN_MS ||
    heartbeatMs > HEARTBEAT_MAX_MS
  ) {
    throw new TypeError(`plugin "${plugin}" presence heartbeatMs must be between 5000 and 120000`)
  }
  if (isReservedFrameworkPath(pathValue)) {
    throw new TypeError(
      `plugin "${plugin}" presence path "${pathValue}" collides with a reserved framework route`,
    )
  }
  return Object.freeze({ id: 'presence@1', plugin, path: pathValue, heartbeatMs })
}
