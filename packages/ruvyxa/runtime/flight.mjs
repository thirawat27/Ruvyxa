/** Fail-closed transport for Ruvyxa server components. */
export const FLIGHT_PROTOCOL = 'ruvyxa.flight'
export const FLIGHT_PROTOCOL_VERSION = 1
export const DEFAULT_FLIGHT_LIMIT = 1024 * 1024
const MAX_DEPTH = 64
const MAX_NODES = 10_000

export function clientReference(id, props = {}) {
  if (!/^m_[a-f0-9]{16}$/.test(id)) throw flightError('invalid client reference id')
  return { $type: 'client-reference', id, props: normalizeValue(props) }
}

export function encodeFlightPayload({ manifestVersion, route, tree }) {
  assertVersion(manifestVersion)
  if (typeof route !== 'string' || !route.startsWith('/') || route.length > 2048) {
    throw flightError('route must be a root-relative path')
  }
  return JSON.stringify({
    protocol: FLIGHT_PROTOCOL,
    protocolVersion: FLIGHT_PROTOCOL_VERSION,
    manifestVersion,
    route,
    tree: normalizeValue(tree),
  })
}

export function decodeFlightPayload(
  payload,
  expectedManifestVersion,
  limit = DEFAULT_FLIGHT_LIMIT,
) {
  const bytes = new TextEncoder().encode(payload).byteLength
  if (bytes > limit) throw flightError('payload exceeds the configured byte limit')
  let envelope
  try {
    envelope = JSON.parse(payload)
  } catch {
    throw flightError('payload is not valid JSON')
  }
  if (
    !isPlainObject(envelope) ||
    envelope.protocol !== FLIGHT_PROTOCOL ||
    envelope.protocolVersion !== FLIGHT_PROTOCOL_VERSION
  ) {
    throw flightError('unsupported transport protocol')
  }
  assertVersion(envelope.manifestVersion)
  if (envelope.manifestVersion !== expectedManifestVersion) {
    throw flightError('manifest version mismatch')
  }
  if (typeof envelope.route !== 'string' || !envelope.route.startsWith('/')) {
    throw flightError('invalid route')
  }
  return {
    manifestVersion: envelope.manifestVersion,
    route: envelope.route,
    tree: normalizeValue(envelope.tree),
  }
}

export function publicFlightError(error, route) {
  const code = /^RUV\d{4}$/.test(error?.code ?? '') ? error.code : 'RUV1830'
  return encodeFlightPayload({
    manifestVersion: '0000000000000000',
    route: typeof route === 'string' && route.startsWith('/') ? route : '/',
    tree: { $type: 'error', code, message: 'Server component rendering failed' },
  })
}

function normalizeValue(root) {
  let nodes = 0
  const ancestors = new Set()
  const visit = (value, depth) => {
    nodes += 1
    if (nodes > MAX_NODES) throw flightError('payload contains too many values')
    if (depth > MAX_DEPTH) throw flightError('payload nesting is too deep')
    if (value === null || typeof value === 'string' || typeof value === 'boolean') return value
    if (typeof value === 'number') {
      if (!Number.isFinite(value)) throw flightError('non-finite numbers are not serializable')
      return value
    }
    if (typeof value !== 'object') throw flightError(`unsupported ${typeof value} value`)
    if (ancestors.has(value)) throw flightError('cyclic values are not serializable')
    ancestors.add(value)
    let normalized
    if (Array.isArray(value)) {
      normalized = value.map((child) => visit(child, depth + 1))
    } else {
      if (!isPlainObject(value)) throw flightError('only plain objects are serializable')
      normalized = Object.create(null)
      for (const key of Object.keys(value).sort()) {
        if (key === '__proto__' || key === 'prototype' || key === 'constructor') {
          throw flightError(`unsafe object key ${JSON.stringify(key)}`)
        }
        normalized[key] = visit(value[key], depth + 1)
      }
    }
    ancestors.delete(value)
    return normalized
  }
  return visit(root, 0)
}

function assertVersion(version) {
  if (!/^[a-f0-9]{16}$/.test(version)) throw flightError('invalid manifest version')
}

function isPlainObject(value) {
  if (!value || typeof value !== 'object') return false
  const prototype = Object.getPrototypeOf(value)
  return prototype === Object.prototype || prototype === null
}

function flightError(message) {
  const error = new TypeError(`RUV1830 Flight: ${message}`)
  error.code = 'RUV1830'
  return error
}
