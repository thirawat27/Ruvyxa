import { notFound } from '@ruvyxa/react'

export default function CatchAll({ params }: Readonly<{ params: { slug: string[] } }>) {
  // This segment has no `not-found.tsx` of its own, so the call resolves to the
  // root boundary at `app/not-found.tsx` — the other half of the nearest-
  // boundary rule that `showcase/` demonstrates with a local one.
  if (params.slug[0] === 'missing') {
    notFound()
  }

  return (
    <main className="page">
      <p className="eyebrow">Catch-all route segment</p>
      <h1>Catch-all: /{params.slug.join('/')}</h1>
      <p>
        Rendered from the <code>catchall/{'[...slug]'}/page.tsx</code> file.
      </p>
      <p>
        The <code>{'[...slug]'}</code> pattern captures all remaining URL segments:
      </p>
      <pre>params = {JSON.stringify(params, null, 2)}</pre>
      <p className="link-row">
        <span>Try:</span>
        <a href="/catchall/one">/catchall/one</a>
        <a href="/catchall/one/two">/catchall/one/two</a>
        <a href="/catchall/missing">/catchall/missing — root not-found.tsx</a>
      </p>
    </main>
  )
}
