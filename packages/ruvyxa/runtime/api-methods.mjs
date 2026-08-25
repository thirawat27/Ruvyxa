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
