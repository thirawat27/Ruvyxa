import { findItem, store } from '../store'

// A handler receives the request and the matched route parameters. `params.id`
// is the `[id]` folder name; a catch-all segment would give an array, so it is
// typed as both until it has been checked.
type Params = { id?: string | string[] }

/**
 * GET /api/items/:id
 * Read one item.
 */
export function GET({ params }: { params: Params }) {
  const item = typeof params.id === 'string' ? findItem(params.id) : undefined
  if (!item) return Response.json({ error: 'Item not found.' }, { status: 404 })

  return Response.json({ item })
}

/**
 * PUT /api/items/:id
 * Update an item. `name` is required; `description` defaults to empty.
 */
export async function PUT({ request, params }: { request: Request; params: Params }) {
  const item = typeof params.id === 'string' ? findItem(params.id) : undefined
  if (!item) return Response.json({ error: 'Item not found.' }, { status: 404 })

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

  item.name = name.trim()
  item.description = typeof description === 'string' ? description.trim() : ''
  item.updatedAt = new Date().toISOString()

  return Response.json({ item })
}

/**
 * DELETE /api/items/:id
 * Delete an item.
 */
export function DELETE({ params }: { params: Params }) {
  const index =
    typeof params.id === 'string' ? store.items.findIndex((item) => item.id === params.id) : -1
  if (index === -1) return Response.json({ error: 'Item not found.' }, { status: 404 })

  store.items.splice(index, 1)

  return Response.json({ message: 'Item deleted.' })
}
