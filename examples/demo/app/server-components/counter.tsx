'use client'

import { useState, useTransition } from 'react'

import { describeRelease } from './actions'

/**
 * The interactive island on a server-components page.
 *
 * This module is the only part of the route that reaches the browser. The page
 * beside it never does — see `page.tsx` — and neither does `actions.ts`: the
 * `describeRelease` imported above is a *reference*, and calling it posts the
 * argument to the server and resolves to what the real function returned there.
 *
 * `count` is the same thing written the other way: it is declared inside
 * `page.tsx` with an inline `'use server'` and handed down as an ordinary prop,
 * arriving here through the payload. Both spellings produce a reference, and
 * calling either posts to the server.
 */
export default function Counter({
  start,
  count,
}: {
  start: number
  count: (channel: string) => Promise<number>
}) {
  const [value, setValue] = useState(start)
  const [answer, setAnswer] = useState<{ version: string; note: string } | null>(null)
  const [total, setTotal] = useState<number | null>(null)
  const [pending, startTransition] = useTransition()

  return (
    <p className="counter-row">
      <button
        type="button"
        className="counter"
        disabled={pending}
        onClick={() => {
          const next = value + 1
          setValue(next)
          startTransition(async () => {
            setAnswer(await describeRelease(next))
            setTotal(await count('stable'))
          })
        }}
      >
        clicked {value} times
      </button>{' '}
      <span className="counter-total">
        {answer === null
          ? 'no server function called yet'
          : `server answered ${answer.version} (${answer.note}) of ${total} stable releases`}
      </span>
    </p>
  )
}
