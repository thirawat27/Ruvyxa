export default function Home() {
  return (
    <main>
      <h1>CRUD Starter</h1>
      <p>
        A CRUD application demonstrating server actions, data loading, and form handling with
        Ruvyxa. Navigate to <a href="/tasks">Tasks</a> to see it in action.
      </p>
      <div className="card">
        <h2>What's included</h2>
        <ul>
          <li>
            <strong>Server Actions</strong> — Mutations via <code>action.input().handler()</code>
          </li>
          <li>
            <strong>Data Loaders</strong> — Read data with <code>loader()</code> and{' '}
            <code>cache()</code>
          </li>
          <li>
            <strong>Form Handling</strong> — Progressive enhancement with server-side validation
          </li>
          <li>
            <strong>File-system Routing</strong> — Pages map to the <code>app/</code> directory
          </li>
        </ul>
      </div>
      <p>
        Edit <code>app/page.tsx</code> to customize this page. Check <a href="/about">About</a> for
        architecture details.
      </p>
    </main>
  )
}
