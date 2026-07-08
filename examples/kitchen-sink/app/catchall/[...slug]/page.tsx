export default function CatchAll({ params }: { params: { slug: string } }) {
  return (
    <main className="page">
      <p className="eyebrow">Catch-all route segment</p>
      <h1>Catch-all: /{params.slug ?? ""}</h1>
      <p>Rendered from the <code>catchall/{'[...slug]'}/page.tsx</code> file.</p>
      <p>The <code>{'[...slug]'}</code> pattern captures all remaining URL segments:</p>
      <pre>params = {JSON.stringify(params, null, 2)}</pre>
      <p className="link-row">
        <span>Try:</span>
        <a href="/catchall/one">/catchall/one</a>
        <a href="/catchall/one/two">/catchall/one/two</a>
      </p>
    </main>
  )
}
