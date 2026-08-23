import { Link } from '@ruvyxa/react'
import type { Meta } from '@ruvyxa/react'

import './globals.css'

// Root metadata. Every `meta` on a route's path merges from here down to the
// page, so `titleTemplate` formats the title of every route below it.
export const meta: Meta = {
  title: 'My Ruvyxa Blog',
  titleTemplate: '%s — My Ruvyxa Blog',
  description: 'A content-focused blog built with Ruvyxa.',
  lang: 'en',
}

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body>
        <header className="header">
          <nav className="nav" aria-label="Main navigation">
            <Link href="/" className="nav-brand">
              My Blog
            </Link>
            <ul className="nav-links">
              <li>
                <Link href="/">Home</Link>
              </li>
              <li>
                <Link href="/blog">Blog</Link>
              </li>
              <li>
                <Link href="/about">About</Link>
              </li>
            </ul>
          </nav>
        </header>
        <main className="content">{children}</main>
        <footer className="footer">
          <p>&copy; My Ruvyxa Blog. Built with Ruvyxa.</p>
        </footer>
      </body>
    </html>
  )
}
