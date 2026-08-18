import './globals.css'

export const meta = {
  title: 'Ruvyxa API Starter',
  description: 'An API-first backend starter built with Ruvyxa.',
}

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
