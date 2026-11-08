/**
 * ISR (Incremental Static Regeneration) — Revalidates every 60 seconds.
 *
 * This page is pre-rendered at build time like SSG, but the cached HTML
 * is refreshed in the background after the revalidation interval expires.
 *
 * Detection: `export const revalidate = 60` → ISR with 60s TTL.
 */

// ISR configuration: revalidate every 60 seconds
export const revalidate = 60

export default function IsrPage() {
  // Impure on purpose: the render time is what this page demonstrates, and a
  // revalidated copy is told apart from the cached one only by it.
  // oxlint-disable-next-line react/purity
  const now = new Date().toISOString()

  return (
    <main className="page-wide">
      <h1>ISR: Incremental Static Regeneration</h1>
      <p>
        {/* The server stamps this at render time and the client stamps it again
            at hydration, so the two never match. That difference is the point of
            the demo, and `suppressHydrationWarning` is how React is told a
            mismatch here is intended rather than a bug. */}
        This page was rendered at: <code suppressHydrationWarning>{now}</code>
      </p>
      <p>
        It will be served from cache for up to <strong>60 seconds</strong>, then revalidated in the
        background on the next request.
      </p>

      <section>
        <h2>How it works</h2>
        <ol>
          <li>First request: page is rendered and cached</li>
          <li>Subsequent requests within 60s: served from cache instantly</li>
          <li>After 60s: stale content is served, background revalidation triggers</li>
          <li>Next request after revalidation: gets the fresh content</li>
        </ol>
      </section>

      <section>
        <h2>When to use ISR</h2>
        <ul>
          <li>E-commerce product pages (prices change occasionally)</li>
          <li>News articles (comments/reactions update)</li>
          <li>Dashboard summaries (refresh every few minutes)</li>
        </ul>
      </section>

      <p className="badge">Strategy: ISR (revalidate: 60s)</p>
    </main>
  )
}
