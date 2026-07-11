import { Image } from '@ruvyxa/react'

export default function Home() {
  return (
    <main className="page-wide">
      <Image
        className="logo"
        src="/ruvyxa.png"
        alt="Ruvyxa fox logo"
        width={120}
        height={120}
        priority
      />
      <h1>Ruvyxa Framework Demo</h1>
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

      <h2>Rendering Strategies</h2>
      <div className="feature-grid">
        <a className="feature-card" href="/static-page">
          <h3>SSG (Static)</h3>
          <p>Pre-rendered at build time</p>
          <code>No Node.js at runtime</code>
        </a>
        <a className="feature-card" href="/ssg-blog">
          <h3>SSG (Dynamic)</h3>
          <p>getStaticParams + [slug]</p>
          <code>Build-time params</code>
        </a>
        <a className="feature-card" href="/isr-page">
          <h3>ISR</h3>
          <p>Revalidates every 60s</p>
          <code>Stale-while-revalidate</code>
        </a>
        <a className="feature-card" href="/csr-page">
          <h3>CSR</h3>
          <p>"use client" — browser only</p>
          <code>No SSR overhead</code>
        </a>
        <a className="feature-card" href="/ppr-page">
          <h3>PPR</h3>
          <p>Static shell + streaming</p>
          <code>Best of both worlds</code>
        </a>
      </div>
    </main>
  )
}
