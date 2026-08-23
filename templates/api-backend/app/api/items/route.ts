import { badRequest, optionalString, readJsonObject } from '../http'
import { nextItemId, store, type Item } from './store'

/**
 * GET /api/items — list every item.
 */
export function GET(): Response {
  const items = [...store.items.values()]
  return Response.json({ items, count: items.length })
}

/**
 * POST /api/items — create an item.
 *
 * Requires `name`; `description` is optional. Answers `201` with the created
 * item and a `Location` header pointing at it, which is what lets a client
 * follow the resource without guessing how ids are built.
 */
export async function POST({ request }: { request: Request }): Promise<Response> {
  const body = await readJsonObject(request)
  if (body instanceof Response) return body

  const name = optionalString(body, 'name', 200)
  if (name instanceof Response) return name
  if (!name) return badRequest('Field "name" is required and must be a non-empty string.')

  const description = optionalString(body, 'description', 2000)
  if (description instanceof Response) return description

  const now = new Date().toISOString()
  const item: Item = {
    id: nextItemId(),
    name,
    description: description ?? '',
    createdAt: now,
    updatedAt: now,
  }
  store.items.set(item.id, item)

  return Response.json({ item }, { status: 201, headers: { Location: `/api/items/${item.id}` } })
}
