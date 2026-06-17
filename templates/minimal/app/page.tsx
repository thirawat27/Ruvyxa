export default function Home() {
  return (
    <main className="page">
      <img className="logo" src="/ruvyxa.png" alt="Ruvyxa logo" width="132" height="132" />
      <h1>Hello Ruvyxa</h1>
      <p>Full-stack TypeScript, powered by Rust.</p>
      <nav>
        <a href="/about">About</a>
        <a href="/todos">Todos</a>
      </nav>
    </main>
  )
}
