import { Link } from '@ruvyxa/react'

export default function Showcase() {
  return (
    <main className="page">
      <p className="eyebrow">Special files</p>
      <h1>Showcase</h1>
      <p>
        This section wires <code>loading.tsx</code>, <code>error.tsx</code>, and{' '}
        <code>not-found.tsx</code> around its pages — the Next.js special-file conventions.
      </p>
      <ul>
        <li>
          <Link href="/showcase/widgets">/showcase/widgets — an item that exists</Link>
        </li>
        <li>
          <Link href="/showcase/missing">/showcase/missing — calls notFound()</Link>
        </li>
        <li>
          <Link href="/showcase/boom">/showcase/boom — throws, caught by error.tsx</Link>
        </li>
      </ul>
    </main>
  )
}
