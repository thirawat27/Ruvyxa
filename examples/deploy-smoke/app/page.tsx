import { Link } from '@ruvyxa/react'

export const meta = {
  title: 'Ruvyxa deployment smoke',
  description: 'A pre-rendered page, so the adapter copies a file and the server serves it.',
}

/**
 * The pre-rendered half of the smoke.
 *
 * A static page is a file the adapter copied, so serving it exercises the
 * publish directory and its cache headers rather than the render path. The
 * `<Link>` is here to give the route a client bundle: without one the emitted
 * server would never be asked for a hashed asset under `/__ruvyxa/client/`.
 */
export default function Page() {
  return (
    <main>
      <h1>Ruvyxa deployment smoke</h1>
      <p data-smoke="page">This page was pre-rendered at build time.</p>
      <Link href="/cached">Revalidated page</Link>
    </main>
  )
}
