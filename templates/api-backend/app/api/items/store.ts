export interface Item {
  id: string
  name: string
  description: string
  createdAt: string
  updatedAt: string
}

interface ItemStore {
  items: Map<string, Item>
  nextId: number
}

const runtime = globalThis as typeof globalThis & { __RUVYXA_API_ITEMS__?: ItemStore }
export const store = (runtime.__RUVYXA_API_ITEMS__ ??= {
  items: new Map<string, Item>([
    [
      'item_1',
      {
        id: 'item_1',
        name: 'Example Item',
        description: 'This is a pre-seeded example item.',
        createdAt: new Date(Date.now() - 86_400_000).toISOString(),
        updatedAt: new Date(Date.now() - 86_400_000).toISOString(),
      },
    ],
  ]),
  nextId: 2,
})

export function nextItemId(): string {
  return `item_${store.nextId++}`
}
