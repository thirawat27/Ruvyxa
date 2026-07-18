import { store } from '../store'

type ItemRouteContext = {
  request: Request
  params: { id?: string | string[] }
}

/**
 * GET /api/items/:id
 * Get a single item by its ID.
 */
export async function GET({ params }: Pick<ItemRouteContext, 'params'>) {
  const { id } = params

  if (!id || typeof id !== 'string') {
    return Response.json(
      { error: 'Parameter "id" is required.', status: 400 },
      { status: 400, headers: { 'Content-Type': 'application/json' } },
    )
  }

  const item = store.items.get(id)
  if (!item) {
    return Response.json(
      { error: 'Not found.', status: 404 },
      { status: 404, headers: { 'Content-Type': 'application/json' } },
    )
  }

  return Response.json({ item }, { status: 200, headers: { 'Content-Type': 'application/json' } })
}

/**
 * PUT /api/items/:id
 * Update an existing item. Accepts partial updates (name and/or description).
 */
export async function PUT({ request, params }: ItemRouteContext) {
  const { id } = params

  if (!id || typeof id !== 'string') {
    return Response.json(
      { error: 'Parameter "id" is required.', status: 400 },
      { status: 400, headers: { 'Content-Type': 'application/json' } },
    )
  }

  const existing = store.items.get(id)
  if (!existing) {
    return Response.json(
      { error: 'Not found.', status: 404 },
      { status: 404, headers: { 'Content-Type': 'application/json' } },
    )
  }

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

  if (name !== undefined) {
    if (typeof name !== 'string' || name.trim().length === 0) {
      return Response.json(
        { error: 'Field "name" must be a non-empty string.', status: 400 },
        { status: 400, headers: { 'Content-Type': 'application/json' } },
      )
    }
    if (name.trim().length > 200) {
      return Response.json(
        { error: 'Field "name" must be 200 characters or fewer.', status: 400 },
        { status: 400, headers: { 'Content-Type': 'application/json' } },
      )
    }
    existing.name = name.trim()
  }

  if (description !== undefined) {
    if (typeof description !== 'string') {
      return Response.json(
        { error: 'Field "description" must be a string.', status: 400 },
        { status: 400, headers: { 'Content-Type': 'application/json' } },
      )
    }
    existing.description = description.trim()
  }

  existing.updatedAt = new Date().toISOString()
  store.items.set(id, existing)

  return Response.json(
    { item: existing },
    { status: 200, headers: { 'Content-Type': 'application/json' } },
  )
}

/**
 * DELETE /api/items/:id
 * Delete an item by its ID.
 */
export async function DELETE({ params }: Pick<ItemRouteContext, 'params'>) {
  const { id } = params

  if (!id || typeof id !== 'string') {
    return Response.json(
      { error: 'Parameter "id" is required.', status: 400 },
      { status: 400, headers: { 'Content-Type': 'application/json' } },
    )
  }

  if (!store.items.has(id)) {
    return Response.json(
      { error: 'Not found.', status: 404 },
      { status: 404, headers: { 'Content-Type': 'application/json' } },
    )
  }

  store.items.delete(id)

  return Response.json(
    { message: 'Item deleted.' },
    { status: 200, headers: { 'Content-Type': 'application/json' } },
  )
}
