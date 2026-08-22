import { Suspense } from 'react'

/**
 * A server-components route whose document is streamed.
 *
 * `force-dynamic` is what makes this page's document a per-request render
 * rather than something written once and stored — and a document produced per
 * request is the only kind that can be streamed, because a cached or
 * pre-rendered one has to become a string somewhere.
 *
 * Each section below waits before it resolves. The shell — everything outside
 * the `Suspense` boundaries — is sent as soon as React has it, and each section
 * follows when the server finishes it. View the page and the fallbacks are on
 * screen while the slow ones are still being awaited; without streaming the
 * whole document would wait for the slowest of them.
 */
export const serverComponents = true
export const dynamic = 'force-dynamic'

export const meta = {
  title: 'Streaming',
  description: 'A server-components document sent while it is still being rendered.',
}

/** Stand-in for a query that takes as long as queries take. */
async function SlowSection({ label, delay }: { label: string; delay: number }) {
  await new Promise((resolve) => setTimeout(resolve, delay))
  return (
    <li>
      <strong>{label}</strong> — resolved after {delay}ms
    </li>
  )
}

export default function StreamingPage() {
  return (
    <main className="page">
      <p className="eyebrow">Streaming</p>
      <h1>Streaming</h1>
      <p>
        This shell reached the browser before either section below had rendered. Each one replaced
        its fallback when the server finished it.
      </p>
      <ul data-streamed-sections>
        <Suspense fallback={<li data-fallback="fast">waiting on the fast section…</li>}>
          <SlowSection label="fast" delay={300} />
        </Suspense>
        <Suspense fallback={<li data-fallback="slow">waiting on the slow section…</li>}>
          <SlowSection label="slow" delay={1200} />
        </Suspense>
      </ul>
      <p className="hint">
        A pre-rendered or cached server-components route is still sent whole: it has to become a
        string to be stored, and this one is never stored.
      </p>
    </main>
  )
}
