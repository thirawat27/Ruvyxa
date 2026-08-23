import { Link } from '@ruvyxa/react'
import type { Meta } from '@ruvyxa/react'

import './globals.css'

export const meta: Meta = {
  title: 'Ruvyxa Full-Stack App',
  titleTemplate: '%s · Ruvyxa',
  description: 'A CRUD application starter built with Ruvyxa.',
  lang: 'en',
}

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body>
        <nav aria-label="Main navigation">
          <span className="brand">Ruvyxa</span>
          <Link href="/">Home</Link>
          <Link href="/tasks">Tasks</Link>
          <Link href="/about">About</Link>
        </nav>
        {children}
      </body>
    </html>
  )
}
