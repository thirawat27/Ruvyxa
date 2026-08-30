/**
 * Which exported function answers a request to a `route.ts`, and what a refusal
 * has to say when none does.
 *
 * Three hosts dispatch API routes and each had its own copy of `mod[method]`
 * followed by a bare 405: `serverless-handler.mjs` for every deployed build,
 * `worker-pool.mjs` for `ruvyxa dev` and `ruvyxa start`, and `api-renderer.mjs`
 * for the isolated single-route path. All three agreed, and all three were
 * wrong in the same two ways — which is what a rule copied three times does.
 *
 * Deliberately free of `node:` imports so an edge bundle can carry it.
 */

/**
 * The methods a route module may export, in the order a refusal lists them.
 *
 * A fixed order rather than a sort: `Allow` is compared byte-for-byte by
 * caches and test fixtures, and `localeCompare` is banned here because it
 * answers by the host's ICU locale.
 */
const METHODS = ['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS']

/**
 * Every method this route answers, including the one it never exports.
 *
 * `HEAD` is `GET` without the content — RFC 9110 §9.3.2 — so a resource that
 * serves `GET` serves `HEAD` by definition, and a handler is not required to
 * write one. Ruvyxa refused it with a 405 instead, which is what an uptime
 * monitor, a link checker, and a CDN revalidation all send first.
 */
export function routeMethods(module) {
  return METHODS.filter(
    (name) =>
      typeof module?.[name] === 'function' ||
      (name === 'HEAD' && typeof module?.GET === 'function'),
  )
}

/**
 * The handler for a method, or `null` when the route does not answer it.
 *
 * `omitBody` is set for a `HEAD` answered by `GET`: the headers are the ones
 * `GET` would send and the content is dropped. Dropped here rather than left to
 * the transport, because only two of the three hosts have a transport — a
 * serverless function hands its `Response` straight to the platform.
 */
export function selectRouteHandler(module, method) {
  const requested = String(method ?? 'GET').toUpperCase()
  if (typeof module?.[requested] === 'function') {
    return { handler: module[requested], method: requested, omitBody: requested === 'HEAD' }
  }
  if (requested === 'HEAD' && typeof module?.GET === 'function') {
    return { handler: module.GET, method: 'GET', omitBody: true }
  }
  return null
}

/**
 * The refusal, with the header RFC 9110 §15.5.6 says a 405 MUST carry.
 *
 * Without `Allow` a client is told only that its method is wrong, never which
 * one to use, and a CORS preflight has nothing to read.
 */
export function methodNotAllowed(module, method) {
  return {
    status: 405,
    allow: routeMethods(module).join(', '),
    body: `Method ${String(method ?? 'GET').toUpperCase()} is not allowed`,
  }
}

/**
 * Turn whatever a route handler returned into a `Response`.
 *
 * One home, reached by all three hosts. This was written out three times — in
 * `worker-pool.mjs`, `api-renderer.mjs` and `serverless-handler.mjs` — including
 * the RUV1504 message text, so the rule a developer reads when their handler
 * returns nothing was maintained in triplicate. It lives here because
 * `api-methods.mjs` is already carried into every function bundle and already
 * imported by all three.
 */
export function normalizeResponse(result, route = 'this route') {
  if (result instanceof Response) return result
  // Returning serialisable data instead of a Response is a supported
  // convenience. Returning nothing is not: `Response.json(undefined)` throws
  // "Value is not JSON serializable" from inside undici, and the message that
  // reached the caller named neither the handler nor the fact that it returned
  // nothing — the suggested fix was to check the module's imports.
  if (result === undefined) {
    throw new Error(
      `RUV1504 the handler for ${route} returned nothing. A route handler must return a Response, ` +
        'or data that can be serialised as JSON, which is sent as `Response.json(data)`.',
    )
  }
  try {
    return Response.json(result)
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new Error(
      `RUV1504 the handler for ${route} returned a value that cannot be serialised as JSON ` +
        `(${detail}). Return a Response, or data built from plain objects, arrays, strings, ` +
        'numbers, booleans, and null.',
    )
  }
}
