import type { Meta } from '@ruvyxa/react'

import './globals.css'

export const meta: Meta = {
  title: 'Ruvyxa API Starter',
  titleTemplate: '%s · Ruvyxa API',
  description: 'An API-first backend starter built with Ruvyxa.',
  lang: 'en',
}

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
