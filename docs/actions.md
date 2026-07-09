# Server Actions

Server actions are typed, validated mutations that run exclusively on the server. They live in `action.ts` files beside pages and are invoked through Ruvyxa's action endpoint.

---

## Defining an Action

```ts
// app/todos/action.ts
import { action } from "ruvyxa/server"

export const createTodo = action
  .input({
    parse(value: unknown) {
      if (!value || typeof value !== "object" || !("title" in value)) {
        throw new Error("Title is required")
      }
      return { title: String(value.title).trim() }
    },
  })
  .handler(async ({ input, invalidate }) => {
    // Persist the todo (database, API, etc.)
    const todo = await db.todos.create({ title: input.title })

    // Invalidate cached data
    invalidate("todos")

    return todo
  })
```

### Anatomy

| Part | Purpose |
|------|---------|
| `action` | Creates a new server action |
| `.input({ parse })` | Validates and transforms the raw input |
| `.handler()` | Executes the mutation logic |
| `input` | The validated, typed input from `.input()` |
| `invalidate(key)` | Marks cached loader data as stale |

---

## Calling Actions

### From HTML forms

```tsx
export default function TodosPage() {
  return (
    <form method="post" action="/__ruvyxa/action?path=/todos&name=createTodo">
      <input name="title" placeholder="New todo" required />
      <button type="submit">Add</button>
    </form>
  )
}
```

### From JavaScript

```ts
const response = await fetch("/__ruvyxa/action?path=/todos&name=createTodo", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ title: "Ship it" }),
})
```

### Endpoint format

```
POST /__ruvyxa/action?path=<route-path>&name=<action-export-name>
```

The endpoint resolves the action from the target route's sibling `action.ts`. Clients cannot specify arbitrary module paths.

---

## Input Validation

The `parse` function receives the raw request body and must return a validated object or throw:

```ts
export const updateUser = action
  .input({
    parse(value: unknown) {
      const obj = value as Record<string, unknown>

      if (!obj.email || typeof obj.email !== "string") {
        throw new Error("Valid email required")
      }
      if (!obj.name || typeof obj.name !== "string") {
        throw new Error("Name required")
      }

      return {
        email: obj.email.toLowerCase().trim(),
        name: obj.name.trim(),
      }
    },
  })
  .handler(async ({ input }) => {
    return db.users.update(input)
  })
```

If `parse` throws, the action returns a structured error response without executing the handler.

---

## Supported Content Types

The action endpoint accepts:

- `application/json` — parsed as JSON
- `application/x-www-form-urlencoded` — parsed as form fields

Other content types are rejected with `415 Unsupported Media Type`.

---

## Security Defaults

Ruvyxa applies multiple layers of protection to every action call:

| Protection | Behavior |
|-----------|----------|
| **Body size limit** | Payloads over 64 KB are rejected with `413` |
| **Content-Type guard** | Only JSON and form-encoded are accepted |
| **Origin validation** | `Origin` must match `Host` header, or `403` |
| **Fetch Metadata** | `Sec-Fetch-Site: cross-site` is rejected with `403` |
| **Rate limiting** | Per-client/action throttling (60 req/min default) |
| **Security headers** | Standard headers applied to all responses |
| **Module isolation** | Actions can only be invoked from their owning route |

These protections work automatically. No configuration needed.

---

## Cache Invalidation

Use `invalidate()` inside your handler to mark specific cache keys as stale:

```ts
.handler(async ({ input, invalidate }) => {
  await db.todos.create({ title: input.title })
  invalidate("todos")        // Single key
  invalidate("dashboard")    // Another key
  return { ok: true }
})
```

Invalidated keys will be refetched on the next loader call that uses them.

---

## Error Handling

Actions return structured responses:

```json
// Success
{ "ok": true, "data": { "title": "My todo", "completed": false } }

// Validation error (parse threw)
{ "ok": false, "error": { "message": "Title is required" } }

// Runtime error (handler threw)
{ "ok": false, "error": { "message": "Database connection failed" } }
```

---

## Diagnostic Codes

| Code | Meaning |
|------|---------|
| `RUV1500` | Action runtime error — validation failure or handler exception |
| `RUV1501` | Route has no `action.ts` or `action.js` file |
| `RUV1502` | Action renderer script not found |
| `RUV1503` | Internal renderer invocation missing arguments — renderer called without required args |

---

## Best Practices

- Keep actions small and focused — one mutation per export.
- Always validate input. Never trust the client payload.
- Use `invalidate()` to keep loaders fresh after mutations.
- Co-locate actions with the page that uses them.
- For complex validation, consider extracting a schema library — but the `parse` function is the enforcement point.

---

## Related

- [Data Loading](data.md) — server-side loaders and caching
- [Routing](routing.md) — file conventions and route structure
- [Debugging](debugging.md) — action diagnostics and tracing
