export default function Home() {
  return (
    <main className="page">
      <section className="hero" aria-labelledby="home-title">
        <img className="logo" src="/ruvyxa.png" alt="Ruvyxa logo" width="112" height="112" />
        <p className="eyebrow">Ruvyxa starter</p>
        <h1 id="home-title">Build a full-stack app with Rust-powered TypeScript.</h1>
        <p className="lead">
          Edit <code>app/page.tsx</code> and run <code>pnpm dev</code> to start building.
        </p>
        <div className="actions" aria-label="Project links">
          <a className="primary" href="https://github.com/thirawat27/ruvyxa">
            Documentation
          </a>
          <a href="https://github.com/thirawat27/ruvyxa/tree/main/examples/basic-app">
            Example app
          </a>
        </div>
      </section>
    </main>
  )
}
