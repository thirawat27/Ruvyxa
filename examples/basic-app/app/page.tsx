export default function Home() {
  return (
    <main className="page">
      <img className="logo" src="/ruvyxa.png" alt="Ruvyxa logo" width="144" height="144" />
      <h1>Hello Ruvyxa</h1>
      <p>File routing, CSS injection, route manifests, and Rust-powered dev serving.</p>
      <nav>
        <a href="/about">About</a>
        <a href="/blog/hello">Dynamic route</a>
        <a href="/todos">Server action</a>
        <a href="/api/health">Health API</a>
      </nav>
    </main>
  )
}
