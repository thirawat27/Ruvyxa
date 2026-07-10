export default function About() {
  return (
    <main className="page">
      <p className="eyebrow">Static nested route</p>
      <h1>About</h1>
      <p>
        Rendered from <code>app/about/page.tsx</code> — a static page with no dynamic parameters.
      </p>
      <p>
        This demonstrates basic file-system routing: every <code>page.tsx</code> file becomes a
        route at its directory path.
      </p>
    </main>
  )
}
