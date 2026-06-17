# Actions

Server actions live in `action.ts` beside a page route and are called through the Ruvyxa action endpoint.

```ts
import { action } from "ruvyxa/server"

export const createTodo = action
  .input({
    parse(value: unknown) {
      if (!value || typeof value !== "object" || !("title" in value)) {
        throw new Error("Todo title is required")
      }

      return { title: String(value.title).trim() }
    },
  })
  .handler(async ({ input, invalidate }) => {
    invalidate("todos")
    return { title: input.title, completed: false }
  })
```

Use a plain HTML form:

```tsx
export default function TodosPage() {
  return (
    <form method="post" action="/__ruvyxa/action?path=/todos&name=createTodo">
      <input name="title" />
      <button type="submit">Create todo</button>
    </form>
  )
}
```

The endpoint accepts JSON or URL-encoded form payloads. It resolves the action from the target route's sibling `action.ts`, so clients cannot choose arbitrary module paths.

## Security Defaults

- Payloads over 64 KiB are rejected with `413` before the action body is extracted.
- Content types must be `application/json` or `application/x-www-form-urlencoded`.
- Requests with an `Origin` that does not match the `Host` header are rejected with `403`.
- Browser Fetch Metadata with `Sec-Fetch-Site: cross-site` is rejected with `403`.
- Action calls are rate-limited per client/action key in the runtime process.
- Action responses include the framework's default security headers.
- Browser-reachable code still cannot import `server/` modules or read private `process.env.*` values.

## Debug Tips

- `RUV1501` means the route has no `action.ts` or `action.js`.
- `RUV1502` means the action renderer could not be found.
- `RUV1503` means the internal renderer invocation is missing required arguments.
- `RUV1500` wraps schema validation and action runtime errors.
