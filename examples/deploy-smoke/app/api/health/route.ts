/**
 * The endpoint the deployment smoke asks for.
 *
 * An API route reaches the generated route registry rather than the publish
 * directory, so answering it proves the adapter compiled the project's own code
 * into the function and that the handler routes to it.
 */
export function GET() {
  return Response.json({ ok: true, framework: 'Ruvyxa' })
}
