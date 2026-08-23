import { badRequest, notFound, optionalString, readJsonObject } from '../../http'
import { store, type Item } from '../store'

/**
 * Handlers receive the matched route parameters alongside the request. A
 * dynamic segment can be captured more than once by a catch-all, so the value
 * is `string | string[]` until it has been narrowed.
 */
interface ItemRouteContext {
  request: Request
  params: { id?: string | string[] }
}

/** Narrow `params.id` to the single segment this route declares, or explain why not. */
function requireId(params: ItemRouteContext['params']): string | Response {
  const { id } = params
  if (typeof id !== 'string' || id === '') return badRequest('Parameter "id" is required.')
  return id
}

/** Look an item up, or answer 404. */
function requireItem(params: ItemRouteContext['params']): Item | Response {
  const id = requireId(params)
  if (id instanceof Response) return id
  return store.items.get(id) ?? notFound(`No item has the id "${id}".`)
}

/**
 * GET /api/items/:id — read one item.
 */
export function GET({ params }: Pick<ItemRouteContext, 'params'>): Response {
  const item = requireItem(params)
  if (item instanceof Response) return item
  return Response.json({ item })
}

/**
 * PATCH /api/items/:id — update the fields the body mentions.
 *
 * `PATCH` rather than `PUT`: this applies a partial change, and `PUT` means
 * "replace the resource with exactly this". Using `PUT` for a partial update is
 * the kind of thing a client only finds out about by losing a field.
 */
export async function PATCH({ request, params }: ItemRouteContext): Promise<Response> {
  const existing = requireItem(params)
  if (existing instanceof Response) return existing

  const body = await readJsonObject(request)
  if (body instanceof Response) return body

  const name = optionalString(body, 'name', 200)
  if (name instanceof Response) return name
  if (name === '') return badRequest('Field "name" must be a non-empty string.')

  const description = optionalString(body, 'description', 2000)
  if (description instanceof Response) return description

  if (name === undefined && description === undefined) {
    return badRequest('Provide at least one of "name" or "description".')
  }

  const updated: Item = {
    ...existing,
    name: name ?? existing.name,
    description: description ?? existing.description,
    updatedAt: new Date().toISOString(),
  }
  store.items.set(updated.id, updated)

  return Response.json({ item: updated })
}

/**
 * DELETE /api/items/:id — remove one item.
 *
 * Answers `204 No Content`: the deletion is the whole answer, and a body
 * restating it is one more thing for a client to parse and ignore.
 */
export function DELETE({ params }: Pick<ItemRouteContext, 'params'>): Response {
  const item = requireItem(params)
  if (item instanceof Response) return item
  store.items.delete(item.id)
  return new Response(null, { status: 204 })
}
