'use client'

import type { RouteErrorProps } from '@ruvyxa/react'

export default function ShowcaseError({ error, reset }: RouteErrorProps) {
  return (
    <main className="page">
      <p className="eyebrow">Error</p>
      <h1>Something went wrong</h1>
      <p>{error.message}</p>
      <button type="button" onClick={reset}>
        Try again
      </button>
    </main>
  )
}
