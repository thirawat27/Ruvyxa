'use client'

import { useState } from 'react'

/**
 * The interactive island on a server-components page.
 *
 * This module is the only part of the route that reaches the browser. The page
 * beside it never does — see `page.tsx`.
 */
export default function Counter({ start }: { start: number }) {
  const [value, setValue] = useState(start)
  return (
    <button type="button" className="counter" onClick={() => setValue(value + 1)}>
      clicked {value} times
    </button>
  )
}
