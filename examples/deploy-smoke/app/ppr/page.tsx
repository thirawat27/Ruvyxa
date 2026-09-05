import { Suspense } from 'react'

export const meta = { title: 'Partially pre-rendered page' }

/**
 * The PPR half of the smoke.
 *
 * `ppr` makes the build write a shell and the emitted server *serve* it — the
 * strategy every deployed host used to render in full on every request while
 * still writing the forced render to a store it never read back. The stamp is
 * what makes that observable over HTTP: a stored shell carries the build's
 * clock and answers the same bytes twice, a per-request render answers a
 * different stamp each time. `suppressHydrationWarning` because the client
 * stamps it again on hydration, by design.
 */
export const ppr = true

function Stamp() {
  // Impure on purpose: the whole check is that a stored shell answers the
  // build's clock twice, and only an impure value can tell that apart from a
  // page rendered again for every request.
  // oxlint-disable-next-line react/purity
  const stamp = String(Date.now())
  return (
    <code data-smoke="ppr-stamp" suppressHydrationWarning>
      {stamp}
    </code>
  )
}

export default function Page() {
  return (
    <main>
      <p data-smoke="ppr">Partially pre-rendered shell.</p>
      <Suspense fallback={<p data-smoke="ppr-fallback">Loading the slot.</p>}>
        <Stamp />
      </Suspense>
    </main>
  )
}
