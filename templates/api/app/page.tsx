export default function ApiDocsPage() {
  return (
    <main>
      <span className="badge">REST API</span>
      <h1>Ruvyxa API Starter</h1>
      <p>
        A small REST API. Each endpoint is a <code>route.ts</code> under <code>app/api/</code>, and
        the folder is the URL.
      </p>

      <h2>Health</h2>

      <article className="endpoint" aria-label="GET /api/health">
        <span className="method get">GET</span>
        <span className="path">/api/health</span>
        <p className="desc">Server status and the current time.</p>
        <pre>
          <code>{`curl http://localhost:3000/api/health`}</code>
        </pre>
      </article>

      <h2>Items</h2>

      <article className="endpoint" aria-label="GET /api/items">
        <span className="method get">GET</span>
        <span className="path">/api/items</span>
        <p className="desc">List all items.</p>
        <pre>
          <code>{`curl http://localhost:3000/api/items`}</code>
        </pre>
      </article>

      <article className="endpoint" aria-label="POST /api/items">
        <span className="method post">POST</span>
        <span className="path">/api/items</span>
        <p className="desc">
          Create an item. <code>name</code> is required.
        </p>
        <pre>
          <code>{`curl -X POST http://localhost:3000/api/items \\
  -H "Content-Type: application/json" \\
  -d '{"name": "Widget", "description": "A useful widget"}'`}</code>
        </pre>
      </article>

      <article className="endpoint" aria-label="GET /api/items/:id">
        <span className="method get">GET</span>
        <span className="path">/api/items/:id</span>
        <p className="desc">Read one item.</p>
        <pre>
          <code>{`curl http://localhost:3000/api/items/1`}</code>
        </pre>
      </article>

      <article className="endpoint" aria-label="PUT /api/items/:id">
        <span className="method put">PUT</span>
        <span className="path">/api/items/:id</span>
        <p className="desc">Update an item.</p>
        <pre>
          <code>{`curl -X PUT http://localhost:3000/api/items/1 \\
  -H "Content-Type: application/json" \\
  -d '{"name": "Super Widget"}'`}</code>
        </pre>
      </article>

      <article className="endpoint" aria-label="DELETE /api/items/:id">
        <span className="method delete">DELETE</span>
        <span className="path">/api/items/:id</span>
        <p className="desc">Delete an item.</p>
        <pre>
          <code>{`curl -X DELETE http://localhost:3000/api/items/1`}</code>
        </pre>
      </article>

      <h2>Errors</h2>
      <p>Every failure answers with the same shape:</p>
      <pre>
        <code>{`{ "error": "Item not found." }`}</code>
      </pre>

      <h2>Next</h2>
      <p>
        <code>app/api/items/store.ts</code> is an in-memory array — it resets on restart. Replace it
        with a database and the route handlers stay as they are.
      </p>
    </main>
  )
}
