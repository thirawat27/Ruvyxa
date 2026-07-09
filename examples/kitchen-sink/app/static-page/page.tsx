/**
 * SSG (Static Site Generation) — Pure static page.
 *
 * This page has no dynamic segments and no data fetching.
 * It will be pre-rendered at build time and served as a static HTML file
 * without invoking Node.js at request time.
 *
 * Detection: no dynamic markers → SSG for static routes.
 * To explicitly opt into SSG for pages with data, export `getStaticParams`.
 */

export default function StaticPage() {
  return (
    <main className="page-wide">
      <h1>SSG: Static Page</h1>
      <p>
        This page was pre-rendered at <strong>build time</strong>. The production
        server serves it directly as an HTML file — no Node.js worker is invoked
        per request.
      </p>

      <section>
        <h2>How it works</h2>
        <ul>
          <li>At build time, Ruvyxa calls <code>ssg-renderer.mjs</code> to render this page</li>
          <li>The output HTML is saved to <code>{'.ruvyxa/prerender/static-page/index.html'}</code></li>
          <li>The production server serves it directly with no runtime cost</li>
        </ul>
      </section>

      <section>
        <h2>When to use SSG</h2>
        <ul>
          <li>Marketing pages, documentation, blog posts</li>
          <li>Content that doesn't change between deployments</li>
          <li>Pages where every user sees the same content</li>
        </ul>
      </section>

      <p className="badge">Strategy: SSG</p>
    </main>
  )
}
