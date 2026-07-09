import './globals.css'

export const meta = {
  title: 'Ruvyxa Kitchen Sink',
  description: 'Comprehensive Ruvyxa framework example',
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
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
              <a href="/env">Env</a>
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
