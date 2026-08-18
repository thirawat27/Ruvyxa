import { Script } from '@ruvyxa/react'
import { cookies, draftMode, headers } from 'ruvyxa/server'

/**
 * Reading the request makes this page per-visitor.
 *
 * `cookies()` here is what tells the server this document belongs to one
 * request, so it is never stored in the shared render cache. Nothing declares
 * that — the call itself is the declaration.
 */
export default function RequestPage() {
  const theme = cookies().get('theme') ?? '(unset)'
  const language = headers().get('accept-language') ?? '(none)'
  const drafting = draftMode().isEnabled

  return (
    <main className="page">
      <p className="eyebrow">Request context</p>
      <h1>Reading the request</h1>
      <p>
        <code>cookies()</code>, <code>headers()</code>, and <code>draftMode()</code> read the
        request being served. Set a <code>theme</code> cookie and reload to see this change.
      </p>

      <table>
        <thead>
          <tr>
            <th>Source</th>
            <th>Value</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>
              <code>cookies().get(&apos;theme&apos;)</code>
            </td>
            <td>{theme}</td>
          </tr>
          <tr>
            <td>
              <code>headers().get(&apos;accept-language&apos;)</code>
            </td>
            <td>{language}</td>
          </tr>
          <tr>
            <td>
              <code>draftMode().isEnabled</code>
            </td>
            <td>{String(drafting)}</td>
          </tr>
        </tbody>
      </table>

      <h2>Third-party scripts</h2>
      <p>
        <code>&lt;Script&gt;</code> keeps a tag off the critical path. This one is inline and{' '}
        <code>beforeInteractive</code>, so it runs before hydration and is visible in view-source.
      </p>
      <Script id="demo-request-marker" strategy="beforeInteractive">
        {`window.__ruvyxaRequestDemo = true`}
      </Script>
    </main>
  )
}
