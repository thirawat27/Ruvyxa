import "./global.css"

export const meta = {
  title: "Basic Ruvyxa App",
  description: "A minimal Ruvyxa example.",
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
