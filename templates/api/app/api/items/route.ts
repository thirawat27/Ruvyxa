import { nextItemId, store, type Item } from './store'

/**
 * GET /api/items
 * List all items.
 */
export function GET() {
  return Response.json({ items: store.items, count: store.items.length })
}

/**
 * POST /api/items
 * Create an item. Requires a JSON body with a `name`.
 */
export async function POST({ request }: { request: Request }) {
  let body: unknown
  try {
    body = await request.json()
  } catch {
    return Response.json({ error: 'Invalid JSON body.' }, { status: 400 })
  }

  const { name, description } = (body ?? {}) as { name?: unknown; description?: unknown }

  if (typeof name !== 'string' || name.trim() === '') {
    return Response.json({ error: 'Field "name" is required.' }, { status: 400 })
  }

  const now = new Date().toISOString()
  const item: Item = {
    id: nextItemId(),
    name: name.trim(),
    description: typeof description === 'string' ? description.trim() : '',
    createdAt: now,
    updatedAt: now,
  }
  store.items.push(item)

  return Response.json({ item }, { status: 201 })
}
