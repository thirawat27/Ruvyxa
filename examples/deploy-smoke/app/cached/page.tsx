export const meta = { title: 'Revalidated page' }

/**
 * The ISR half of the smoke.
 *
 * `revalidate` is what makes the emitted server read the pre-rendered file,
 * decide it is stale, and write a fresh one back — the only route kind that
 * exercises `readPrerendered` and `writePrerendered` together.
 */
export const revalidate = 60

export default function Page() {
  return (
    <main>
      <p data-smoke="cached">Revalidated on a 60 second window.</p>
    </main>
  )
}
