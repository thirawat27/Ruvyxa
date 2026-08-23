export default function ApiDocsPage() {
  return (
    <main>
      <span className="badge">REST API</span>
      <h1>Ruvyxa API Starter</h1>
      <p>
        Route handlers with validation, correct status codes, and one error shape. Every endpoint
        below is a <code>route.ts</code> under <code>app/api/</code>; the folder is the URL.
      </p>

      <h2>Health</h2>

      <article className="endpoint" aria-label="GET /api/health">
        <span className="method get">GET</span>
        <span className="path">/api/health</span>
        <p className="desc">
          Liveness, answered with <code>Cache-Control: no-store</code> so no proxy can serve a stale
          &ldquo;ok&rdquo;.
        </p>
        <pre>
          <code>{`curl http://localhost:3000/api/health`}</code>
        </pre>
        <pre>
          <code>{`{ "status": "ok", "timestamp": "2026-08-23T00:00:00.000Z" }`}</code>
        </pre>
      </article>

      <h2>Items</h2>

      <article className="endpoint" aria-label="GET /api/items">
        <span className="method get">GET</span>
        <span className="path">/api/items</span>
        <p className="desc">List every item.</p>
        <pre>
          <code>{`curl http://localhost:3000/api/items`}</code>
        </pre>
        <pre>
          <code>{`{ "items": [...], "count": 1 }`}</code>
        </pre>
      </article>

      <article className="endpoint" aria-label="POST /api/items">
        <span className="method post">POST</span>
        <span className="path">/api/items</span>
        <p className="desc">
          Create an item. <code>name</code> is required. Answers <code>201</code> with a{' '}
          <code>Location</code> header pointing at the new resource.
        </p>
        <pre>
          <code>{`curl -X POST http://localhost:3000/api/items \\
  -H "Content-Type: application/json" \\
  -d '{"name": "Widget", "description": "A useful widget"}'`}</code>
        </pre>
        <pre>
          <code>{`201 Created
Location: /api/items/9f1c…

{ "item": { "id": "9f1c…", "name": "Widget", "description": "A useful widget", "createdAt": "…", "updatedAt": "…" } }`}</code>
        </pre>
      </article>

      <article className="endpoint" aria-label="GET /api/items/:id">
        <span className="method get">GET</span>
        <span className="path">/api/items/:id</span>
        <p className="desc">
          Read one item. The store is seeded with <code>item_seed</code> so this works on a cold
          start.
        </p>
        <pre>
          <code>{`curl http://localhost:3000/api/items/item_seed`}</code>
        </pre>
        <pre>
          <code>{`{ "item": { "id": "item_seed", "name": "Example Item", ... } }`}</code>
        </pre>
      </article>

      <article className="endpoint" aria-label="PATCH /api/items/:id">
        <span className="method patch">PATCH</span>
        <span className="path">/api/items/:id</span>
        <p className="desc">
          Update the fields the body mentions. <code>PATCH</code> rather than <code>PUT</code>,
          because a partial change is not a replacement.
        </p>
        <pre>
          <code>{`curl -X PATCH http://localhost:3000/api/items/item_seed \\
  -H "Content-Type: application/json" \\
  -d '{"name": "Super Widget"}'`}</code>
        </pre>
        <pre>
          <code>{`{ "item": { "id": "item_seed", "name": "Super Widget", ... } }`}</code>
        </pre>
      </article>

      <article className="endpoint" aria-label="DELETE /api/items/:id">
        <span className="method delete">DELETE</span>
        <span className="path">/api/items/:id</span>
        <p className="desc">
          Remove one item. Answers <code>204 No Content</code> — the deletion is the whole answer.
        </p>
        <pre>
          <code>{`curl -i -X DELETE http://localhost:3000/api/items/item_seed`}</code>
        </pre>
        <pre>
          <code>{`204 No Content`}</code>
        </pre>
      </article>

      <h2>Errors</h2>
      <p>
        Every failure answers with the shape RFC 9457 defines, sent as{' '}
        <code>application/problem+json</code> so a client can tell an error from a result without
        inspecting its fields. The helpers live in <code>app/api/http.ts</code>.
      </p>
      <pre>
        <code>{`404 Not Found
Content-Type: application/problem+json

{ "title": "Not Found", "status": 404, "detail": "No item has the id \\"abc\\"." }`}</code>
      </pre>

      <h2>Where to look next</h2>
      <p>
        <code>app/api/http.ts</code> holds the body parsing, field validation, and error shape that
        every handler shares. <code>app/api/items/store.ts</code> is the in-memory store to replace
        with a database — it resets on restart, and each server process keeps its own copy.
      </p>
    </main>
  )
}
