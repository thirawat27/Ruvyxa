export function GET() {
  return Response.json({
    ok: true,
    framework: 'Ruvyxa',
    version: '1.0.5',
    routes: 9,
  })
}
