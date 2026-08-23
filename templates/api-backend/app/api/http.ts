/**
 * Shared HTTP helpers for this API.
 *
 * `Response.json()` already sets `Content-Type: application/json`, so the only
 * thing worth centralizing is the error shape — and the error shape is worth
 * getting right once, because every client has to parse it.
 */

/** A machine-readable error body, in the shape RFC 9457 defines. */
export interface Problem {
  /** Short, stable, human-readable summary of the problem type. */
  title: string
  /** HTTP status code, repeated in the body so a logged payload stands alone. */
  status: number
  /** What went wrong with *this* request. Safe to show a developer. */
  detail?: string
}

/**
 * Answer with an error.
 *
 * The media type is `application/problem+json` rather than `application/json`:
 * a client can then tell an error body from a success body without inspecting
 * its fields, which is the whole point of the registered type.
 */
export function problem(status: number, title: string, detail?: string): Response {
  const body: Problem = detail === undefined ? { title, status } : { title, status, detail }
  return Response.json(body, {
    status,
    headers: { 'Content-Type': 'application/problem+json' },
  })
}

export const badRequest = (detail: string) => problem(400, 'Bad Request', detail)
export const notFound = (detail = 'No resource matches that identifier.') =>
  problem(404, 'Not Found', detail)

/**
 * Read a JSON object body, or explain why it could not be read.
 *
 * Returns the parsed object on success and a ready-to-return `Response` on
 * failure, so a handler stays a straight line: `if (body instanceof Response)
 * return body`.
 */
export async function readJsonObject(
  request: Request,
): Promise<Record<string, unknown> | Response> {
  let parsed: unknown
  try {
    parsed = await request.json()
  } catch {
    return badRequest('Request body is not valid JSON.')
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return badRequest('Request body must be a JSON object.')
  }
  return parsed as Record<string, unknown>
}

/**
 * Validate one optional string field.
 *
 * Returns `undefined` when the field is absent — which is what makes the same
 * function usable for a create, where absence is a separate error, and for a
 * partial update, where absence means "leave it alone".
 */
export function optionalString(
  body: Record<string, unknown>,
  field: string,
  maxLength: number,
): string | undefined | Response {
  const value = body[field]
  if (value === undefined) return undefined
  if (typeof value !== 'string') return badRequest(`Field "${field}" must be a string.`)
  const trimmed = value.trim()
  if (trimmed.length > maxLength) {
    return badRequest(`Field "${field}" must be ${maxLength} characters or fewer.`)
  }
  return trimmed
}
