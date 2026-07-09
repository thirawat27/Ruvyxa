export default function Home() {
  return (
    <main className="page-wide">
      <h1>Ruvyxa Kitchen Sink</h1>
      <p>Comprehensive example app demonstrating every Ruvyxa framework feature.</p>

      <div className="feature-grid">
        <a className="feature-card" href="/about">
          <h3>Static Route</h3>
          <p>app/about/page.tsx</p>
          <code>GET /about</code>
        </a>
        <a className="feature-card" href="/blog">
          <h3>Blog + Dynamic Routes</h3>
          <p>app/blog/page.tsx + {'[slug]'}</p>
          <code>GET /blog/:slug</code>
        </a>
        <a className="feature-card" href="/catchall/a/b/c">
          <h3>Catch-all Route</h3>
          <p>app/catchall/{'[...slug]'}/page.tsx</p>
          <code>GET /catchall/*</code>
        </a>
        <a className="feature-card" href="/api/health">
          <h3>API Route</h3>
          <p>app/api/health/route.ts</p>
          <code>GET /api/health</code>
        </a>
        <a className="feature-card" href="/todos">
          <h3>Server Action</h3>
          <p>app/todos/action.ts</p>
          <code>POST /__ruvyxa/action</code>
        </a>
        <a className="feature-card" href="/env">
          <h3>Environment Variables</h3>
          <p>app/env/page.tsx</p>
          <code>RUVYXA_PUBLIC_*</code>
        </a>
      </div>
    </main>
  )
}

