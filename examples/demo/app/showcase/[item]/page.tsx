import { notFound } from '@ruvyxa/react'

const ITEMS: Record<string, string> = {
  widgets: 'A tidy pile of widgets.',
  gadgets: 'Assorted gadgets, some assembled.',
}

export default function ShowcaseItem({ params }: { params: { item: string } }) {
  const item = params.item

  // Demonstrates error.tsx: an uncaught throw is caught by the nearest boundary.
  if (item === 'boom') {
    throw new Error('Intentional showcase error from /showcase/boom')
  }

  // Demonstrates not-found.tsx: notFound() renders the nearest not-found.tsx.
  const description = ITEMS[item]
  if (!description) {
    notFound()
  }

  return (
    <main className="page">
      <p className="eyebrow">Showcase item</p>
      <h1>{item}</h1>
      <p>{description}</p>
    </main>
  )
}
