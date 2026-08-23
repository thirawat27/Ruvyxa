export interface Item {
  id: string
  name: string
  description: string
  createdAt: string
  updatedAt: string
}

// In-memory data store — resets on server restart, and each server process
// keeps its own copy. Replace it with a database in a real application.
//
// The id counter lives beside the items rather than being derived from
// `items.length`, because deleting an item would then hand the next create an
// id that already exists.
interface Store {
  items: Item[]
  nextId: number
}

const now = new Date().toISOString()
const runtime = globalThis as typeof globalThis & { __RUVYXA_API_ITEMS__?: Store }

export const store = (runtime.__RUVYXA_API_ITEMS__ ??= {
  items: [
    {
      id: '1',
      name: 'Example Item',
      description: 'A pre-seeded item, so every example below returns something.',
      createdAt: now,
      updatedAt: now,
    },
  ],
  nextId: 2,
})

export function findItem(id: string): Item | undefined {
  return store.items.find((item) => item.id === id)
}

export function nextItemId(): string {
  return String(store.nextId++)
}
