'use client'

import { useState } from 'react'

import { echo } from './actions'

/**
 * The client half of the server-components smoke.
 *
 * A hook is the point: a client component rendered by the SSR pass with a
 * React that is not the one `react-dom/server` is driving throws
 * `Cannot read properties of null (reading 'useState')`, which is how a
 * deployed bundle carrying more than one React copy announces itself. Rendering
 * the initial count into the markup is what lets the smoke check it over HTTP.
 *
 * The second button calls a server function, which is a different endpoint and
 * a different failure: `POST /__ruvyxa/rsc`, which every deployed build refused
 * with a `405` until the emitted handler learned the verb. Over HTTP that looked
 * like nothing at all — the document was correct and its status was 200. In a
 * browser the click threw `Connection closed.` and left a blank page, which is
 * why this button exists and why the smoke calls the same endpoint directly.
 */
export function Counter() {
  const [count, setCount] = useState(0)
  const [answer, setAnswer] = useState('unknown')
  return (
    <>
      <button type="button" data-smoke="counter" onClick={() => setCount(count + 1)}>
        count {count}
      </button>
      <button
        type="button"
        data-smoke="echo"
        onClick={async () => setAnswer(await echo(`clicked${count}`))}
      >
        echo {answer}
      </button>
    </>
  )
}
