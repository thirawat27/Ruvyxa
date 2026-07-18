export default function ApiDocsPage() {
  return (
    <main>
      <span className="badge">REST API</span>
      <h1>Ruvyxa API Starter</h1>
      <p>
        An API-first backend demonstrating REST route handlers with validation, proper HTTP status
        codes, and JSON responses.
      </p>

      <h2>Health Check</h2>

      <article className="endpoint" aria-label="GET /api/health">
        <span className="method get">GET</span>
        <span className="path">/api/health</span>
        <p className="desc">Returns server health status and current timestamp.</p>
        <pre>
          <code>{`curl http://localhost:3000/api/health`}</code>
        </pre>
        <pre>
          <code>{`{ "status": "ok", "timestamp": "2025-01-01T00:00:00.000Z" }`}</code>
        </pre>
      </article>

      <h2>Items</h2>

      <article className="endpoint" aria-label="GET /api/items">
        <span className="method get">GET</span>
        <span className="path">/api/items</span>
        <p className="desc">List all items in the store.</p>
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
        <p className="desc">Create a new item. Requires a JSON body with a name field.</p>
        <pre>
          <code>{`curl -X POST http://localhost:3000/api/items \\
  -H "Content-Type: application/json" \\
  -d '{"name": "Widget", "description": "A useful widget"}'`}</code>
        </pre>
        <pre>
          <code>{`{ "item": { "id": "abc123", "name": "Widget", "description": "A useful widget", "createdAt": "...", "updatedAt": "..." } }`}</code>
        </pre>
      </article>

      <article className="endpoint" aria-label="GET /api/items/:id">
        <span className="method get">GET</span>
        <span className="path">/api/items/:id</span>
        <p className="desc">Get a single item by its ID.</p>
        <pre>
          <code>{`curl http://localhost:3000/api/items/abc123`}</code>
        </pre>
        <pre>
          <code>{`{ "item": { "id": "abc123", "name": "Widget", ... } }`}</code>
        </pre>
      </article>

      <article className="endpoint" aria-label="PUT /api/items/:id">
        <span className="method put">PUT</span>
        <span className="path">/api/items/:id</span>
        <p className="desc">Update an existing item. Accepts partial updates.</p>
        <pre>
          <code>{`curl -X PUT http://localhost:3000/api/items/abc123 \\
  -H "Content-Type: application/json" \\
  -d '{"name": "Super Widget"}'`}</code>
        </pre>
        <pre>
          <code>{`{ "item": { "id": "abc123", "name": "Super Widget", ... } }`}</code>
        </pre>
      </article>

      <article className="endpoint" aria-label="DELETE /api/items/:id">
        <span className="method delete">DELETE</span>
        <span className="path">/api/items/:id</span>
        <p className="desc">Delete an item by ID.</p>
        <pre>
          <code>{`curl -X DELETE http://localhost:3000/api/items/abc123`}</code>
        </pre>
        <pre>
          <code>{`{ "message": "Item deleted." }`}</code>
        </pre>
      </article>

      <h2>Error Responses</h2>
      <p>All endpoints return consistent error shapes:</p>
      <pre>
        <code>{`{ "error": "Not found.", "status": 404 }`}</code>
      </pre>
    </main>
  )
}
