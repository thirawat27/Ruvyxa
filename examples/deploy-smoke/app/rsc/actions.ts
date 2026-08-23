'use server'

/**
 * A server function the client half calls, and the harder of the two shapes.
 *
 * This module is imported by `counter.tsx`, which is a `'use client'`
 * component — so it is in the *browser* graph and in no other. The
 * `react-server` graph that renders the page never walks it, because a client
 * component is a reference there and a reference's imports are not followed. A
 * deployment that built its action bundle from the server graph alone would
 * answer `RUV1861` for exactly this call.
 *
 * `POST /__ruvyxa/rsc` answered `405` in every deployed build until the emitted
 * handler learned the verb, so clicking the button that calls this threw
 * `Connection closed.` in the browser and blanked the page — while every check
 * the smoke could make over HTTP stayed green.
 *
 * The `server:` prefix is what proves the answer came from here rather than
 * from a client-side fallback: the argument alone would round-trip either way.
 */
export async function echo(value: string): Promise<string> {
  return `server:${value}`
}
