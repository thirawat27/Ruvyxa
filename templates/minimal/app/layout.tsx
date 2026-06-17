import "./global.css"

export const meta = {
  title: "Ruvyxa App",
  description: "A minimal Ruvyxa app.",
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
