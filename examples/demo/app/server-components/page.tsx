import Counter from './counter'
import releases from './releases.json'

/**
 * Render this route through the React Server Components pipeline.
 *
 * The page runs in a module graph resolved with React's `react-server`
 * condition, which has no `useState` and no `createContext` — so the code below
 * cannot accidentally become a client component. What ships to the browser is
 * `counter.tsx` and nothing else: not this module, and not `releases.json`,
 * which is read here and never sent.
 */
export const serverComponents = true

export const meta = {
  title: 'Server Components',
  description: 'A page whose server half never reaches the browser.',
}

/** Stand-in for a query, a file read, or anything else that needs a server. */
async function loadReleases() {
  return releases.entries.filter((entry) => entry.channel === releases.channel)
}

export default async function ServerComponentsPage() {
  // `await` in a component body. A `'use client'` component cannot do this —
  // it renders in the browser, where there is nothing to await against.
  const entries = await loadReleases()

  return (
    <main className="page">
      <p className="eyebrow">React Server Components</p>
      <h1>Server Components</h1>
      <p>
        This page read the <code>{releases.channel}</code> channel out of a JSON module while
        rendering. Look at the page source: the data is in the payload, the module is not in any
        browser bundle, and neither is this component.
      </p>
      <ul>
        {entries.map((entry) => (
          <li key={entry.version}>
            <strong>{entry.version}</strong> — {entry.note}
          </li>
        ))}
      </ul>
      <p>
        The button is a <code>&apos;use client&apos;</code> component, and the only module from this
        route in the browser bundle. The page itself was serialised into a payload instead of being
        shipped.
      </p>
      <Counter start={0} />
      <p className="hint">
        The payload rides in a <code>&lt;script type=&quot;application/json&quot;&gt;</code> data
        block, so no <code>unsafe-inline</code> is needed in a Content-Security-Policy.
      </p>
    </main>
  )
}
