import { Link } from '@ruvyxa/react'
import type { Meta } from '@ruvyxa/react'

import './globals.css'

/**
 * Root metadata.
 *
 * Every `meta` on a route's path merges from here down to the page, most
 * specific wins — so `titleTemplate` formats the title of every route below,
 * including the frontmatter title of each `page.mdx`.
 */
export const meta: Meta = {
  title: 'My Ruvyxa Blog',
  titleTemplate: '%s — My Ruvyxa Blog',
  description: 'A content-focused blog built with Ruvyxa.',
  siteName: 'My Ruvyxa Blog',
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
            {/* `rss.xml` is generated from the posts by `content: true`. It is a
                published file rather than a route, so it stays a plain anchor. */}
            <a className="nav-feed" href="/rss.xml">
              RSS
            </a>
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
