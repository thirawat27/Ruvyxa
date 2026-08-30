import { SITE_FACTS } from '~/lib/site-facts'

export const meta = {
  title: 'About',
  description: 'A static nested route rendered from app/about/page.tsx.',
  canonical: 'https://demo.ruvyxa.dev/about',
}

export default function About() {
  return (
    <main className="page">
      <p className="eyebrow">Static nested route</p>
      <h1>About</h1>
      <p>
        Rendered from <code>app/about/page.tsx</code> — a static page with no dynamic parameters.
      </p>
      <p>
        This demonstrates basic {SITE_FACTS.routingModel} routing: every <code>page.tsx</code> file
        becomes a route at its directory path.
      </p>
      <p>
        The sentence above reads a value from <code>lib/site-facts.ts</code>, imported as{' '}
        <code>{SITE_FACTS.aliasPrefix}lib/site-facts</code> — a tsconfig <code>paths</code> alias,
        resolved by both module graphs, from a module that lives outside <code>app/</code>.
      </p>
    </main>
  )
}
