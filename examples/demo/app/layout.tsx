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
            <a className="brand" href="/">
              Ruvyxa
            </a>
            <div className="nav-links">
              <a href="/">Home</a>
              <a href="/about">About</a>
              <a href="/blog">Blog</a>
              <a href="/todos">Todos</a>
              <a href="/hooks">Hooks</a>
              <a href="/loader">Loader</a>
              <a href="/seo">SEO</a>
              <a href="/env">Env</a>
              <a href="/game">Game</a>
              <a href="/catchall/foo/bar">Catch-all</a>
              <a href="/api/health">API</a>
            </div>
          </nav>
        </header>
        {children}
      </body>
    </html>
  )
}
