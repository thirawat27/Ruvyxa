'use client'

import { useState } from 'react'

import { echo } from './action'

/**
 * The client half of the server-components smoke.
 *
 * A hook is the point: a client component rendered by the SSR pass with a
 * React that is not the one `react-dom/server` is driving throws
 * `Cannot read properties of null (reading 'useState')`, which is how a
 * deployed bundle carrying more than one React copy announces itself. Rendering
 * the initial count into the markup is what lets the smoke check it over HTTP.
 *
 * The `echo` import is what puts a server function in this route's graph. An
 * actions file imported only from a `'use client'` module is invisible to the
 * `react-server` graph — a reference's own imports are never walked — so it is
 * also the case that a build reading one graph alone gets wrong.
 */
export function Counter() {
  const [count, setCount] = useState(0)
  const [answer, setAnswer] = useState('')
  return (
    <>
      <button type="button" data-smoke="counter" onClick={() => setCount(count + 1)}>
        count {count}
      </button>
      <button type="button" data-smoke="echo" onClick={() => echo(String(count)).then(setAnswer)}>
        ask the server
      </button>
      <p data-smoke="answer">{answer || 'no server call yet'}</p>
    </>
  )
}
