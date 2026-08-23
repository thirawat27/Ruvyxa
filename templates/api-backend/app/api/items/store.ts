/**
 * The item store.
 *
 * In-process and therefore per-instance: it resets on restart, and two workers
 * hold two different lists. It exists so the endpoints have something to act on
 * without a database. Replace it with a real client — that client will not
 * resolve in a browser, which is one more reason this module is only ever
 * imported by `route.ts` files.
 */
export interface Item {
  id: string
  name: string
  description: string
  createdAt: string
  updatedAt: string
}

interface ItemStore {
  items: Map<string, Item>
}

/** A stable id, so the documented `GET /api/items/:id` example works on a cold start. */
export const SEED_ITEM_ID = 'item_seed'

const seededAt = new Date(Date.now() - 86_400_000).toISOString()
const runtime = globalThis as typeof globalThis & { __RUVYXA_API_ITEMS__?: ItemStore }

export const store = (runtime.__RUVYXA_API_ITEMS__ ??= {
  items: new Map<string, Item>([
    [
      SEED_ITEM_ID,
      {
        id: SEED_ITEM_ID,
        name: 'Example Item',
        description: 'A pre-seeded item, so every documented request has something to return.',
        createdAt: seededAt,
        updatedAt: seededAt,
      },
    ],
  ]),
})

/**
 * A fresh identifier.
 *
 * `crypto.randomUUID()` rather than an incrementing counter: a counter is only
 * unique inside one process, and this API is meant to be run behind more than
 * one of them.
 */
export function nextItemId(): string {
  return crypto.randomUUID()
}
