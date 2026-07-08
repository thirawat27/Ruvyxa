import "./globals.css"

export const meta = {
  title: "Ruvyxa Kitchen Sink",
  description: "Comprehensive Ruvyxa framework example",
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>
        <nav className="nav">
          <a href="/">Home</a>
          <a href="/about">About</a>
          <a href="/blog">Blog</a>
          <a href="/todos">Todos</a>
          <a href="/env">Env</a>
          <a href="/catchall/foo/bar">Catch-all</a>
          <a href="/api/health">API</a>
        </nav>
        {children}
      </body>
    </html>
  )
}
