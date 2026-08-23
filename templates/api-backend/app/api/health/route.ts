/**
 * GET /api/health — liveness, with the time the answer was produced.
 *
 * `Cache-Control: no-store` is the point of a health check: a cached "ok" from
 * a proxy is exactly the answer you must not receive.
 */
export function GET(): Response {
  return Response.json(
    { status: 'ok', timestamp: new Date().toISOString() },
    { headers: { 'Cache-Control': 'no-store' } },
  )
}
