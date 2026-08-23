'use server'

/**
 * A server function the smoke can call over HTTP.
 *
 * `POST /__ruvyxa/rsc` is the endpoint the browser reaches when a client
 * component calls one of these, and it answered `405` in every deployed build
 * until the generated route registry learned to load an action bundle. Nothing
 * over HTTP said so: the page rendered, hydrated, and returned `200` — the
 * failure was a rejected promise inside the browser, which is why the smoke
 * calls the endpoint itself rather than trusting the document.
 *
 * The reply is a Flight payload rather than JSON, so the answer below arrives
 * as an encoded row the browser decodes with the same React that rendered the
 * page.
 */
export async function echo(value: string): Promise<string> {
  return `server:${value}`
}
