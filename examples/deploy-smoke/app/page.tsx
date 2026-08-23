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
 *
 * The second one is aimed at `/rsc` on purpose. A soft navigation into a
 * server-components route asks the deployed function for `/__ruvyxa/rsc` and
 * renders the answer without a document load, which is the half of that
 * endpoint no status code can prove: the payload has to be readable by the
 * browser's React, not merely served with a plausible content type.
 */
export default function Page() {
  return (
    <main>
      <h1>Ruvyxa deployment smoke</h1>
      <p data-smoke="page">This page was pre-rendered at build time.</p>
      <Link href="/cached">Revalidated page</Link>
      <Link href="/rsc">Server components</Link>
    </main>
  )
}
