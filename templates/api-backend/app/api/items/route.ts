import { nextItemId, store, type Item } from './store'

/**
 * GET /api/items
 * List all items in the store.
 */
export async function GET() {
  const all = Array.from(store.items.values())
  return Response.json(
    { items: all, count: all.length },
    {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    },
  )
}

/**
 * POST /api/items
 * Create a new item. Requires a JSON body with at least a `name` field.
 */
export async function POST({ request }: { request: Request }) {
  let body: unknown
  try {
    body = await request.json()
  } catch {
    return Response.json(
      { error: 'Invalid JSON body.', status: 400 },
      { status: 400, headers: { 'Content-Type': 'application/json' } },
    )
  }

  if (!body || typeof body !== 'object') {
    return Response.json(
      { error: 'Request body must be a JSON object.', status: 400 },
      { status: 400, headers: { 'Content-Type': 'application/json' } },
    )
  }

  const { name, description } = body as Record<string, unknown>

  if (!name || typeof name !== 'string' || name.trim().length === 0) {
    return Response.json(
      { error: 'Field "name" is required and must be a non-empty string.', status: 400 },
      { status: 400, headers: { 'Content-Type': 'application/json' } },
    )
  }

  if (name.trim().length > 200) {
    return Response.json(
      { error: 'Field "name" must be 200 characters or fewer.', status: 400 },
      { status: 400, headers: { 'Content-Type': 'application/json' } },
    )
  }

  const desc = typeof description === 'string' ? description.trim() : ''
  const now = new Date().toISOString()
  const item: Item = {
    id: nextItemId(),
    name: name.trim(),
    description: desc,
    createdAt: now,
    updatedAt: now,
  }

  store.items.set(item.id, item)

  return Response.json({ item }, { status: 201, headers: { 'Content-Type': 'application/json' } })
}
