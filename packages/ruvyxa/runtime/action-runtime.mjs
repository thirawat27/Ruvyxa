/**
 * Shared server-action execution and request validation.
 *
 * Server actions used to exist only inside the Rust host: `worker-pool.mjs`
 * carried the execution half and `crates/ruvyxa_dev_server/src/action_security.rs`
 * carried the validation half, and the standalone/serverless handler had
 * neither. A deployed build therefore answered `POST /__ruvyxa/action` with a
 * 404 — every form in the `crud` template, in `examples/demo`, and in the
 * markup `ruvyxa add` generates, silently broke the moment it left `ruvyxa dev`.
 *
 * The fix is one implementation with two callers rather than a third copy, so
 * this module holds the parts that are host-independent: content-type
 * negotiation, payload parsing, CSRF checks, realtime metadata, and the call
 * into the action itself.
 *
 * Deliberately free of `node:` imports and of `Buffer`. It is copied verbatim
 * into function bundles next to `serverless-handler.mjs`, including bundles
 * targeting edge runtimes where neither exists.
 *
 * The validation order and outcomes below deliberately track
 * `validate_action_request` / `validate_action_payload` in
 * `crates/ruvyxa_dev_server/src/action_security.rs`; changing one without the
 * other makes a request that is accepted locally fail in production, or worse,
 * the reverse. `tests/fixtures/framework-endpoint-conformance.json` records
 * that both hosts must serve this endpoint, but not yet the field-by-field
 * table — do not treat it as covering these rules.
 */

import { collectRevalidations, requestContext, runWithRequestContext } from './request-context.mjs'

/** Payload encodings a server action accepts. Mirrors `action_content_type`. */
export const ACTION_CONTENT_TYPES = Object.freeze([
  'application/json',
  'application/x-www-form-urlencoded',
])

/** Stable content-bound action module identity shared with the native host. */
export function actionReferenceId(routeId, source) {
  let hash = 0xcbf29ce484222325n
  for (const byte of new TextEncoder().encode(`${routeId}\0${source}`)) {
    hash ^= BigInt(byte)
    hash = BigInt.asUintN(64, hash * 0x100000001b3n)
  }
  return `a_${hash.toString(16).padStart(16, '0')}`
}

/** Most channels one action may name, matching the Rust validator. */
const MAX_REALTIME_CHANNELS = 16
/** Longest realtime channel name. */
const MAX_REALTIME_CHANNEL_LENGTH = 128
/** Longest `invalidate()` key retained in realtime metadata. */
const MAX_INVALIDATED_KEY_LENGTH = 256
/** Most `invalidate()` keys retained in realtime metadata. */
const MAX_INVALIDATED_KEYS = 64
/** Longest request path retained in realtime metadata. */
const MAX_REALTIME_PATH_LENGTH = 2048

/**
 * Resolve the declared payload encoding, or `null` when it is not supported.
 *
 * Only the media type is compared; parameters such as `; charset=utf-8` are
 * dropped, which is what a browser sends for a plain `<form method="post">`.
 */
export function actionContentType(headers) {
  const declared = headers.get('content-type')
  if (typeof declared !== 'string') return null
  const mediaType = declared.split(';')[0]?.trim().toLowerCase()
  return ACTION_CONTENT_TYPES.includes(mediaType) ? mediaType : null
}

/** True when `Sec-Fetch-Site` explicitly reports a cross-site request. */
export function actionFetchSiteIsCrossSite(headers) {
  const site = headers.get('sec-fetch-site')
  return typeof site === 'string' && site.toLowerCase() === 'cross-site'
}

/**
 * True when the request is not provably same-origin.
 *
 * Mirrors `action_origin_is_cross_site`. With no `Origin`, a `Sec-Fetch-Site:
 * same-origin` header is the only accepted substitute — failing closed keeps a
 * stripped-origin cross-site form from reaching a mutation endpoint.
 *
 * The scheme is compared only when `X-Forwarded-Proto` states it. Unlike the
 * Rust host there is no transport peer address to weigh that header against:
 * a deployed function is reachable only through its platform's ingress, so the
 * ingress is the trusted proxy by construction. The load-bearing check either
 * way is the host comparison, which a cross-site page cannot forge.
 */
export function actionOriginIsCrossSite(headers) {
  const origin = headers.get('origin')
  if (typeof origin !== 'string' || origin === '') {
    const site = headers.get('sec-fetch-site')
    return !(typeof site === 'string' && site.toLowerCase() === 'same-origin')
  }

  const host = headers.get('host')
  if (typeof host !== 'string' || host === '') return true

  const separator = origin.indexOf('://')
  if (separator < 0) return true
  const originScheme = origin.slice(0, separator)
  const originHost = origin.slice(separator + 3)
  if (originHost === '' || originHost.includes('/')) return true
  if (originHost.toLowerCase() !== host.toLowerCase()) return true

  const forwarded = forwardedScheme(headers)
  return forwarded === null ? false : originScheme.toLowerCase() !== forwarded
}

function forwardedScheme(headers) {
  const value = headers.get('x-forwarded-proto')
  if (typeof value !== 'string') return null
  const scheme = value.split(',')[0]?.trim().toLowerCase()
  return scheme === 'https' || scheme === 'http' ? scheme : null
}

/**
 * Reject a request that must not reach an action, or return `null` to proceed.
 *
 * Ordering matches `validate_action_request`: size, then encoding, then the two
 * CSRF checks. `payloadBytes` is the already-measured body length; the caller
 * has to read the body anyway to build the payload.
 */
export function validateActionRequest(headers, payloadBytes, policy = {}) {
  const limit = Number.isFinite(policy.actionLimit) ? policy.actionLimit : 1024 * 1024
  if (payloadBytes > limit) {
    return textResponse(413, 'Action payload is too large')
  }
  if (actionContentType(headers) === null) {
    return textResponse(415, 'Action payload must be JSON or URL-encoded form data')
  }
  if (policy.sameOrigin !== false && actionOriginIsCrossSite(headers)) {
    return textResponse(403, 'Cross-origin action request blocked')
  }
  if (policy.fetchMeta !== false && actionFetchSiteIsCrossSite(headers)) {
    return textResponse(403, 'Cross-site action request blocked')
  }
  return null
}

/**
 * Validate the raw payload text against its declared encoding.
 *
 * Returns `{contentType, payload}` or `{response}` with the failure to send.
 * An empty JSON body becomes `{}` so a `fetch()` with no body still reaches an
 * action whose input schema has only optional fields — the same allowance the
 * Rust validator makes.
 */
export function validateActionPayload(headers, payloadText) {
  const contentType = actionContentType(headers)
  if (contentType === null) {
    return {
      response: textResponse(415, 'Action payload must declare JSON or URL-encoded form data'),
    }
  }
  const payload = payloadText === '' && contentType === 'application/json' ? '{}' : payloadText
  if (contentType === 'application/json') {
    try {
      JSON.parse(payload)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      return { response: textResponse(400, `Action JSON payload is malformed: ${message}`) }
    }
  }
  return { contentType, payload }
}

/**
 * Decode an action payload into the value handed to the action's input schema.
 *
 * A payload that wraps its value under `input` is unwrapped, which is what the
 * client helper sends; a bare object is passed through, which is what a plain
 * HTML form produces.
 */
export function parseActionPayload(payloadText, contentType) {
  let parsed
  if (contentType === 'application/json') {
    parsed = JSON.parse(payloadText || '{}')
  } else if (contentType === 'application/x-www-form-urlencoded') {
    parsed = Object.fromEntries(new URLSearchParams(payloadText || ''))
  } else {
    try {
      parsed = JSON.parse(payloadText || '{}')
    } catch {
      parsed = Object.fromEntries(new URLSearchParams(payloadText))
    }
  }
  if (parsed && typeof parsed === 'object' && 'input' in parsed) {
    return parsed.input
  }
  return parsed
}

/** Wrap a plain action return value in the documented JSON envelope. */
export function normalizeActionResult(result, invalidated) {
  if (result instanceof Response) return result
  return Response.json({ data: result, invalidated })
}

/** True when `value` is an exported Ruvyxa action rather than a plain function. */
export function isActionExport(value) {
  return typeof value === 'function' && value.ruvyxa?.kind === 'action'
}

/**
 * Build the realtime metadata a successful action publishes, or `null`.
 *
 * Throws on malformed channel metadata rather than dropping it: a typo in a
 * channel name must fail the action, not silently publish to nobody.
 */
export function actionRealtimeEvent(action, actionName, requestPath, invalidated) {
  const configured = action.ruvyxa?.realtime
  if (!configured) return null
  if (!Array.isArray(configured.channels) || configured.channels.length > MAX_REALTIME_CHANNELS) {
    throw new TypeError(`Action ${actionName} has invalid realtime channel metadata`)
  }
  const channels = configured.channels.map((channel) => {
    if (
      typeof channel !== 'string' ||
      channel.length === 0 ||
      channel.length > MAX_REALTIME_CHANNEL_LENGTH ||
      !/^[A-Za-z0-9:._/-]+$/.test(channel)
    ) {
      throw new TypeError(`Action ${actionName} has invalid realtime channel metadata`)
    }
    return channel
  })
  const pathname = new URL(requestPath, 'http://ruvyxa.local').pathname
  return {
    version: 1,
    type: 'action',
    channels: channels.length > 0 ? channels : [realtimeRouteChannel(pathname)],
    action: actionName,
    path: pathname.slice(0, MAX_REALTIME_PATH_LENGTH),
    invalidated: invalidated
      .filter((key) => typeof key === 'string' && key.length <= MAX_INVALIDATED_KEY_LENGTH)
      .slice(0, MAX_INVALIDATED_KEYS),
  }
}

/** Readable channel for a route, hashed once it would exceed the name limit. */
export function realtimeRouteChannel(pathname) {
  const readable = `route:${pathname}`
  if (readable.length <= MAX_REALTIME_CHANNEL_LENGTH) return readable
  let hash = 0xcbf29ce484222325n
  for (const character of pathname) {
    hash ^= BigInt(character.codePointAt(0))
    hash = BigInt.asUintN(64, hash * 0x100000001b3n)
  }
  return `route-hash:${hash.toString(16).padStart(16, '0')}`
}

/**
 * Base64url without `Buffer`.
 *
 * The realtime event travels in a response header, so it has to survive both
 * the Node host and an edge runtime that has no `Buffer`. `TextEncoder` and
 * `btoa` exist in every runtime Ruvyxa targets.
 */
export function encodeRealtimeEvent(event) {
  const bytes = new TextEncoder().encode(JSON.stringify(event))
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '')
}

/**
 * Execute one server action and return its finished `Response`.
 *
 * The caller supplies the already-imported module so each host keeps its own
 * module-loading strategy: the worker compiles and imports on demand, while a
 * function bundle resolves from a compiled registry.
 *
 * Returns `{response, revalidate}` — `revalidate` is the list of paths the
 * action passed to `revalidatePath()`, which each host applies to its own
 * cache.
 */
export async function runAction({
  module,
  actionName,
  payload,
  contentType,
  requestPath,
  headerPairs,
}) {
  const action = module[actionName]
  if (!isActionExport(action)) {
    return {
      response: Response.json({ error: `Action ${actionName} was not found` }, { status: 404 }),
      revalidate: [],
    }
  }

  const input = parseActionPayload(payload, contentType)
  const invalidated = []
  const pairs = Array.isArray(headerPairs) ? headerPairs : []
  const request = new Request(`http://localhost${requestPath}`, {
    method: 'POST',
    headers: pairs,
    body: contentType === 'application/x-www-form-urlencoded' ? payload : JSON.stringify(input),
  })
  const context = requestContext({ headerPairs: pairs, method: 'POST', url: requestPath })
  const result = await runWithRequestContext(context, () =>
    action(input, {
      request,
      invalidate(key) {
        invalidated.push(key)
      },
    }),
  )

  // An action must not be able to forge the framework's own realtime header,
  // so any incoming value is dropped before the authentic one is attached.
  const produced = normalizeActionResult(result, invalidated)
  const headers = new Headers(produced.headers)
  headers.delete('x-ruvyxa-realtime-event')
  const response = new Response(produced.body, {
    status: produced.status,
    statusText: produced.statusText,
    headers,
  })

  const event =
    response.status >= 200 && response.status < 400
      ? actionRealtimeEvent(action, actionName, requestPath, invalidated)
      : null
  if (event) {
    response.headers.set('x-ruvyxa-realtime-event', encodeRealtimeEvent(event))
  }

  return { response, revalidate: collectRevalidations(context) }
}

function textResponse(status, message) {
  return new Response(message, {
    status,
    headers: { 'content-type': 'text/plain; charset=utf-8' },
  })
}
