export function GET() {
  return Response.json({
    ok: true,
    framework: "Ruvyxa",
    version: "1.0.4",
    routes: 8,
  })
}
