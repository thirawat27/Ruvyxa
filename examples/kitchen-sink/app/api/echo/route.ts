export async function POST(request: Request) {
  const body = await request.json()
  return Response.json({
    method: "POST",
    path: "/api/echo",
    body,
    timestamp: new Date().toISOString(),
  })
}
