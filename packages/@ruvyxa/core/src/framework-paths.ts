/**
 * The paths the framework owns, and the rule a configured socket transport
 * has to satisfy.
 *
 * `config.realtime` and `config.collab` in `ruvyxa.config.ts` name the path
 * the Axum host registers a WebSocket route on. Two hosts read that config and
 * cannot share code: the renderer that validates it at config time (this
 * module, copied into `packages/ruvyxa/runtime/framework-paths.mjs` by
 * `pnpm --filter ruvyxa sync:runtime`, so it imports nothing) and the Axum
 * server, which re-checks what it is handed because a bad path there does not
 * produce a diagnostic — it panics matchit inside `Router::route`. Both are
 * held to `transportPaths` in `tests/fixtures/framework-endpoint-conformance.json`.
 */

/**
 * Paths the framework answers itself.
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

/** Whether a path is one the framework answers and a project may not take. */
export function isReservedFrameworkPath(value: string): boolean {
  return RESERVED_FRAMEWORK_PATHS.includes(value)
}

/**
 * Whether a transport path is a literal route.
 *
 * The Axum host registers this string on its router, so any character that
 * router assigns a meaning to is a wildcard rather than a path. A denylist is
 * only correct while it tracks the router's syntax (axum 0.8 captures are
 * `{name}`, not the `?`/`#`/`*` of 0.7), so the rule is an allowlist: one or
 * more `/`-prefixed segments of RFC 3986 unreserved characters, which is a
 * literal path in every router version and can never acquire a meaning.
 *
 * The twin of `is_literal_transport_path` in
 * `crates/ruvyxa_dev_server/src/lib.rs`.
 */
export function isLiteralTransportPath(value: unknown): value is string {
  return typeof value === 'string' && /^(\/[A-Za-z0-9._~-]+)+$/.test(value)
}

/** What a refused transport path is told it may contain. */
export const TRANSPORT_PATH_RULE =
  'must be an exact absolute path of `/`-prefixed segments containing only letters, digits, `-`, `.`, `_`, or `~`'

/**
 * The bounds a heartbeat has to fall inside, shared by both transports.
 *
 * Below five seconds a heartbeat is indistinguishable from traffic and costs
 * more than the connection it is checking; above two minutes an intermediary
 * has usually dropped the socket before the next one arrives.
 */
export const HEARTBEAT_MIN_MS = 5_000
export const HEARTBEAT_MAX_MS = 120_000

/**
 * Buffered broadcast messages one realtime channel may hold.
 *
 * Named for the same reason the heartbeat bounds are: the Axum host re-checks
 * this range, so it is a two-language rule. Both halves wrote it as a literal
 * on one side or the other, which put it outside
 * `scripts/check-cross-language-constants.mjs` — that gate can only see a name
 * declared in both. `transportBounds` in
 * `tests/fixtures/framework-endpoint-conformance.json` is what actually holds
 * the four numbers.
 */
export const REALTIME_CAPACITY_MIN = 16
/** The upper end of {@link REALTIME_CAPACITY_MIN}'s range. */
export const REALTIME_CAPACITY_MAX = 4_096
/** The channel capacity a `realtime` block that names none is given. */
export const REALTIME_CAPACITY_DEFAULT = 256
/** The heartbeat a transport block that names none is given. */
export const HEARTBEAT_DEFAULT_MS = 25_000

/** The `realtime` block as the hosts read it: every field decided. */
export interface NormalizedRealtime {
  readonly path: string
  readonly heartbeatMs: number
  readonly capacity: number
}

/** The `collab` block as the hosts read it: every field decided. */
export interface NormalizedCollab {
  readonly path: string
  readonly heartbeatMs: number
}

function transportBlock(value: unknown, key: string): Record<string, unknown> | undefined {
  if (value === undefined || value === false) return undefined
  if (value === true) return {}
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`RUV1602 config.${key} must be true or an options object.`)
  }
  return value as Record<string, unknown>
}

function transportPath(value: unknown, key: string, fallback: string): string {
  const path = value ?? fallback
  if (!isLiteralTransportPath(path)) {
    throw new TypeError(`RUV1602 config.${key}.path ${TRANSPORT_PATH_RULE}.`)
  }
  if (isReservedFrameworkPath(path)) {
    throw new TypeError(
      `RUV1602 config.${key}.path "${path}" collides with a reserved framework route.`,
    )
  }
  return path
}

function heartbeat(value: unknown, key: string): number {
  const heartbeatMs = value ?? HEARTBEAT_DEFAULT_MS
  if (
    typeof heartbeatMs !== 'number' ||
    !Number.isInteger(heartbeatMs) ||
    heartbeatMs < HEARTBEAT_MIN_MS ||
    heartbeatMs > HEARTBEAT_MAX_MS
  ) {
    throw new TypeError(
      `RUV1602 config.${key}.heartbeatMs must be an integer between ${HEARTBEAT_MIN_MS} and ${HEARTBEAT_MAX_MS}.`,
    )
  }
  return heartbeatMs
}

/**
 * Validate `config.realtime`, or throw naming the field and the rule.
 *
 * `undefined` and `false` turn the transport off; `true` takes every default.
 */
export function normalizeRealtimeConfig(value: unknown): NormalizedRealtime | undefined {
  const block = transportBlock(value, 'realtime')
  if (!block) return undefined
  const capacity = block.capacity ?? REALTIME_CAPACITY_DEFAULT
  if (
    typeof capacity !== 'number' ||
    !Number.isInteger(capacity) ||
    capacity < REALTIME_CAPACITY_MIN ||
    capacity > REALTIME_CAPACITY_MAX
  ) {
    throw new TypeError(
      `RUV1602 config.realtime.capacity must be an integer between ${REALTIME_CAPACITY_MIN} and ${REALTIME_CAPACITY_MAX}.`,
    )
  }
  return Object.freeze({
    path: transportPath(block.path, 'realtime', '/__ruvyxa/realtime'),
    heartbeatMs: heartbeat(block.heartbeatMs, 'realtime'),
    capacity,
  })
}

/** Validate `config.collab`, or throw naming the field and the rule. */
export function normalizeCollabConfig(value: unknown): NormalizedCollab | undefined {
  const block = transportBlock(value, 'collab')
  if (!block) return undefined
  return Object.freeze({
    path: transportPath(block.path, 'collab', '/__ruvyxa/collab'),
    heartbeatMs: heartbeat(block.heartbeatMs, 'collab'),
  })
}
