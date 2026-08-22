import { Link } from '@ruvyxa/react'

import './globals.css'
import '../styles/external.css'
import '../styles/config-entry.css'

export const meta = {
  title: 'Ruvyxa Kitchen Sink',
  titleTemplate: '%s · Ruvyxa Kitchen Sink',
  description: 'Comprehensive Ruvyxa framework example',
  siteName: 'Ruvyxa Kitchen Sink',
  lang: 'en',
}

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body>
        <header className="site-header">
          <nav className="nav" aria-label="Example routes">
            <Link className="brand" href="/">
              Ruvyxa
            </Link>
            <div className="nav-links">
              <Link href="/">Home</Link>
              <Link href="/about">About</Link>
              <Link href="/blog">Blog</Link>
              <Link href="/todos">Todos</Link>
              <Link href="/hooks">Hooks</Link>
              <Link href="/loader">Loader</Link>
              <Link href="/seo">SEO</Link>
              <Link href="/env">Env</Link>
              <Link href="/game">Game</Link>
              <Link href="/server-components">RSC</Link>
              <Link href="/streaming">Streaming</Link>
              <Link href="/catchall/foo/bar">Catch-all</Link>
              {/* An API route, not a page: there is no client bundle to load
                  and a document request is the only thing that can answer it. */}
              <a href="/api/health">API</a>
            </div>
          </nav>
        </header>
        {children}
      </body>
    </html>
  )
}
