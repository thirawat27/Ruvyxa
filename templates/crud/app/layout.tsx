import './globals.css'

export const meta = {
  title: 'Ruvyxa Full-Stack App',
  description: 'A CRUD application starter built with Ruvyxa.',
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>
        <nav aria-label="Main navigation">
          <span className="brand">Ruvyxa</span>
          <a href="/">Home</a>
          <a href="/tasks">Tasks</a>
          <a href="/about">About</a>
        </nav>
        {children}
      </body>
    </html>
  )
}
