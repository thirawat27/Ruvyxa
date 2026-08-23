import { Link } from '@ruvyxa/react'
import type { Meta } from '@ruvyxa/react'

export const meta: Meta = {
  title: 'About',
  description: 'How this starter is put together, and what to replace first.',
}

export default function AboutPage() {
  return (
    <main>
      <h1>About</h1>
      <p>
        This starter is a task list, chosen because it needs everything a real application needs: a
        read path, a write path, validation, and a page that still works with JavaScript disabled.
      </p>

      <section className="card" aria-labelledby="arch-title">
        <h2 id="arch-title">Architecture</h2>
        <dl>
          <dt>
            <strong>Server actions</strong>
          </dt>
          <dd>
            Mutations live in <code>app/tasks/action.ts</code>, built with{' '}
            <code>action.input(schema).handler()</code>. The schema only needs a synchronous{' '}
            <code>parse(value)</code>, so the same validation runs whether the call arrives from a
            plain form post or from client-side JavaScript.
          </dd>

          <dt>
            <strong>Data loading</strong>
          </dt>
          <dd>
            <code>app/tasks/server.ts</code> reads through <code>loader()</code> and{' '}
            <code>cache()</code> from <code>ruvyxa/server</code>. Each action calls{' '}
            <code>invalidate(&apos;tasks&apos;)</code>, so the next read is fresh rather than served
            from a cache the write just made wrong.
          </dd>

          <dt>
            <strong>Forms that work without JavaScript</strong>
          </dt>
          <dd>
            Every control on <Link href="/tasks">Tasks</Link> is a real <code>&lt;form&gt;</code>{' '}
            posting to <code>/__ruvyxa/action</code>. Disable JavaScript and the page keeps working
            — the server runs the action and answers with a new document.
          </dd>

          <dt>
            <strong>Server/client boundary</strong>
          </dt>
          <dd>
            A module importing from <code>ruvyxa/server</code>, or marked <code>server-only</code>,
            must not reach the browser. The bundler enforces that and <code>ruvyxa check</code>{' '}
            reports a violation before a deploy does.
          </dd>
        </dl>
      </section>

      <section className="card" aria-labelledby="next-title">
        <h2 id="next-title">Replace these first</h2>
        <ul>
          <li>
            The in-memory store in <code>app/tasks/server.ts</code> — it resets on restart, and each
            server process holds its own copy. Point it at a database.
          </li>
          <li>
            The hand-written <code>parse()</code> guards in <code>app/tasks/action.ts</code> — any
            schema library with a synchronous <code>parse</code> drops straight in.
          </li>
          <li>
            The absence of authentication. Tasks here belong to everyone; real ones belong to a
            user.
          </li>
          <li>
            <code>site.url</code> in <code>ruvyxa.config.ts</code>, so the build can publish a
            sitemap instead of only <code>robots.txt</code>.
          </li>
        </ul>
      </section>
    </main>
  )
}
