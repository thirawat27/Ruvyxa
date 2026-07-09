import { Suspense } from 'react'

export const ppr = true

async function DynamicSection() {
  const timestamp = new Date().toISOString()
  return (
    <div className="dynamic-slot">
      <h3>Dynamic Content (streamed at request time)</h3>
      <p>
        Current time: <code>{timestamp}</code>
      </p>
      <p>This section was rendered on the server at request time and streamed to the client.</p>
    </div>
  )
}

function DynamicFallback() {
  return (
    <div className="dynamic-slot loading">
      <h3>Loading dynamic content...</h3>
      <div className="skeleton" />
    </div>
  )
}

export default function PprPage() {
  return (
    <main className="page-wide">
      <h1>PPR: Partial Pre-Rendering</h1>
      <p>
        The static parts of this page (header, navigation, layout) were pre-rendered at{' '}
        <strong>build time</strong>. Dynamic sections stream in at request time.
      </p>

      <section>
        <h2>Static Section (pre-rendered)</h2>
        <p>This content is part of the static shell. It never changes between requests.</p>
      </section>

      <Suspense fallback={<DynamicFallback />}>
        <DynamicSection />
      </Suspense>

      <section>
        <h2>How it works</h2>
        <ol>
          <li>Build time: static shell rendered (everything outside Suspense boundaries)</li>
          <li>Request time: dynamic slots rendered and streamed into the shell</li>
          <li>Client: hydrates the combined result</li>
        </ol>
      </section>

      <section>
        <h2>When to use PPR</h2>
        <ul>
          <li>Product pages: static layout + dynamic price/availability</li>
          <li>Social feeds: static shell + personalized content</li>
          <li>Dashboards: cached layout + live metrics</li>
        </ul>
      </section>

      <p className="badge">Strategy: PPR</p>
    </main>
  )
}
