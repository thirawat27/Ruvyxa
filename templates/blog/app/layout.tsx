import './globals.css'

export const meta = {
  title: 'My Ruvyxa Blog',
  description: 'A content-focused blog built with Ruvyxa.',
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>
        <header className="header">
          <nav className="nav" aria-label="Main navigation">
            <a href="/" className="nav-brand">
              My Blog
            </a>
            <ul className="nav-links">
              <li>
                <a href="/">Home</a>
              </li>
              <li>
                <a href="/blog">Blog</a>
              </li>
              <li>
                <a href="/about">About</a>
              </li>
            </ul>
          </nav>
        </header>
        <main className="content">{children}</main>
        <footer className="footer">
          <p>&copy; {new Date().getFullYear()} My Ruvyxa Blog. All rights reserved.</p>
        </footer>
      </body>
    </html>
  )
}
