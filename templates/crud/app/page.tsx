import { Link } from '@ruvyxa/react'

export default function Home() {
  return (
    <main>
      <h1>CRUD Starter</h1>
      <p>
        A task list built the way Ruvyxa builds one: the page renders on the server, the mutations
        are server actions, and the forms work before any JavaScript has loaded. Open{' '}
        <Link href="/tasks">Tasks</Link> to see it.
      </p>
      <div className="card">
        <h2>What&rsquo;s included</h2>
        <ul>
          <li>
            <strong>Server actions</strong> — validated mutations via{' '}
            <code>action.input().handler()</code>
          </li>
          <li>
            <strong>Data loaders</strong> — reads through <code>loader()</code> and{' '}
            <code>cache()</code>, invalidated by the action that changed them
          </li>
          <li>
            <strong>Forms without JavaScript</strong> — every button is a real form post, so the
            page works while the bundle is still downloading
          </li>
          <li>
            <strong>File-system routing</strong> — a folder under <code>app/</code> with a{' '}
            <code>page.tsx</code> is a route, and <code>typedRoutes</code> checks every{' '}
            <code>&lt;Link href&gt;</code> against the ones that exist
          </li>
        </ul>
      </div>
      <p>
        Edit <code>app/page.tsx</code> to change this page. <Link href="/about">About</Link>{' '}
        describes how the pieces fit together and what to replace first.
      </p>
    </main>
  )
}
