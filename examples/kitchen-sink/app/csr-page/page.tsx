"use client"

import { useState, useEffect } from "react"

export default function CsrPage() {
  const [count, setCount] = useState(0)
  const [mounted, setMounted] = useState(false)

  useEffect(() => {
    setMounted(true)
  }, [])

  return (
    <main className="page-wide">
      <h1>CSR: Client-Side Rendering</h1>
      <p>
        This page rendered entirely in the browser. The server sent only a
        minimal HTML shell — no React was executed on the server.
      </p>

      {mounted && (
        <div className="csr-content">
          <section>
            <h2>Interactive Counter</h2>
            <p>Count: <strong>{count}</strong></p>
            <button onClick={() => setCount(c => c + 1)}>Increment</button>
            <button onClick={() => setCount(0)}>Reset</button>
          </section>

          <section>
            <h2>How it works</h2>
            <ul>
              <li>Server sends a minimal shell with an empty root <code>div</code></li>
              <li>Client bundle loads and renders the full React tree</li>
              <li>No SSR overhead — ideal for highly interactive pages</li>
            </ul>
          </section>

          <section>
            <h2>When to use CSR</h2>
            <ul>
              <li>Admin dashboards behind authentication</li>
              <li>Real-time collaborative editors</li>
              <li>Pages where SEO is not a concern</li>
              <li>Heavy client-side interactivity (canvas, WebGL)</li>
            </ul>
          </section>

          <p className="badge">Strategy: CSR</p>
        </div>
      )}
    </main>
  )
}
