import "./global.css"

export const meta = {
  title: "Ruvyxa App",
  description: "Full-stack TypeScript at Rust speed.",
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
