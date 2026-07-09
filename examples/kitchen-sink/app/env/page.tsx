export default function EnvPage() {
  return (
    <main className="page">
      <p className="eyebrow">Environment variables</p>
      <h1>Public Env Vars</h1>
      <p>
        Environment variables prefixed with <code>RUVYXA_PUBLIC_</code> are available in client
        bundles.
      </p>
      <p>Private variables (no prefix) can only be used in server-only modules.</p>

      <table>
        <thead>
          <tr>
            <th>Variable</th>
            <th>Value</th>
            <th>Scope</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>
              <code>RUVYXA_PUBLIC_APP_NAME</code>
            </td>
            <td>{import.meta.env?.RUVYXA_PUBLIC_APP_NAME ?? '(not set)'}</td>
            <td>public</td>
          </tr>
          <tr>
            <td>
              <code>RUVYXA_PUBLIC_API_URL</code>
            </td>
            <td>{import.meta.env?.RUVYXA_PUBLIC_API_URL ?? '(not set)'}</td>
            <td>public</td>
          </tr>
        </tbody>
      </table>

      <h2>Usage</h2>
      <pre>{`# .env
RUVYXA_PUBLIC_APP_NAME=KitchenSink
RUVYXA_PUBLIC_API_URL=https://api.example.com
DATABASE_URL=postgres://...   # private, server-only`}</pre>
    </main>
  )
}
