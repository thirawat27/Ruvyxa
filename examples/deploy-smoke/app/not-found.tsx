/**
 * The project's own not-found page.
 *
 * Present so every deployment lane can assert that an unmatched URL is answered
 * with the page the application wrote rather than a host's generic 404. Each
 * host reaches it differently: a function reads `notFoundDocument` out of the
 * manifest, and a static publish directory has only the `404.html` convention.
 */
export default function NotFound() {
  return (
    <main data-smoke="not-found">
      <h1>Not found</h1>
      <p>SMOKE-NOT-FOUND-MARKER</p>
    </main>
  )
}
