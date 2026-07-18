/**
 * GET /api/health
 * Returns server health status and current timestamp.
 */
export async function GET() {
  return Response.json(
    {
      status: 'ok',
      timestamp: new Date().toISOString(),
    },
    {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    },
  )
}
