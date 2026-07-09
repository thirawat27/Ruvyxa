export default function Home() {
  return (
    <main className="page">
      <section className="main" aria-labelledby="home-title">
        <img className="logo" src="/ruvyxa.png" alt="Ruvyxa logo" width="80" height="80" />
        <h1 className="title" id="home-title">
          Create Ruvyxa App
        </h1>
        <p className="description">
          Edit <code>app/page.tsx</code> to start building your application.
        </p>
        <div className="links">
          <a className="link primary" href="https://github.com/thirawat27/ruvyxa">
            Docs
          </a>
          <a
            className="link"
            href="https://github.com/thirawat27/ruvyxa/tree/main/examples/kitchen-sink"
          >
            Examples
          </a>
        </div>
      </section>
    </main>
  )
}
