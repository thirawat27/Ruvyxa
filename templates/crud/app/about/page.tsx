export default function AboutPage() {
  return (
    <main>
      <h1>About</h1>
      <p>
        This starter demonstrates the full-stack capabilities of Ruvyxa with a task management CRUD
        application.
      </p>

      <section className="card" aria-labelledby="arch-title">
        <h2 id="arch-title">Architecture</h2>
        <dl>
          <dt>
            <strong>Server Actions</strong>
          </dt>
          <dd>
            Mutations are defined in <code>app/tasks/action.ts</code> using{' '}
            <code>action.input().handler()</code>. They run exclusively on the server and are
            callable from forms without client-side JavaScript.
          </dd>

          <dt>
            <strong>Data Loading</strong>
          </dt>
          <dd>
            Data is fetched in <code>app/tasks/server.ts</code> using <code>loader()</code> and{' '}
            <code>cache()</code> from <code>ruvyxa/server</code>. Loaders run before render and
            their results are cached per-request.
          </dd>

          <dt>
            <strong>File-system Routing</strong>
          </dt>
          <dd>
            Each folder under <code>app/</code> with a <code>page.tsx</code> becomes a route.
            Layouts nest automatically via <code>layout.tsx</code>.
          </dd>

          <dt>
            <strong>Server/Client Boundary</strong>
          </dt>
          <dd>
            Files importing from <code>ruvyxa/server</code> are server-only. The bundler enforces
            that server modules are never shipped to the browser.
          </dd>
        </dl>
      </section>

      <section className="card" aria-labelledby="next-title">
        <h2 id="next-title">Next Steps</h2>
        <ul>
          <li>Replace the in-memory store with a database</li>
          <li>Add input validation with a schema library</li>
          <li>Add authentication and per-user tasks</li>
          <li>Deploy with an adapter (Node, Vercel, Cloudflare, etc.)</li>
        </ul>
      </section>
    </main>
  )
}
