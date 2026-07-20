# Plugins

Ruvyxa plugins are ordinary application modules written in TypeScript.

Create a starter:

```bash
npx ruvyxa plugin new auth
```

The command creates `auth/` (named after the plugin — no `--dir` flag needed) with `src/index.ts`,
`package.json`, `tsconfig.json`, and `README.md`. Add `--dir <path>` only if you want a different
location. Plugins run on both Node.js and Bun (`--runtime bun` or `RUVYXA_RUNTIME=bun`):

```ts
import { plugin } from 'ruvyxa/config'

export default plugin('auth', {
  routes: ['/*'],
  onRequest(request) {
    const headers = new Headers(request.headers)
    headers.set('x-auth', 'true')
    return new Request(request, { headers })
  },
})
```

Import it from `ruvyxa.config.ts`:

```ts
import auth from './plugins/auth'
import { config } from 'ruvyxa/config'

export default config({ plugins: [auth] })
```

Use `plugin(name, middleware)` for request/response middleware. It accepts either a middleware
object (with optional `routes`, `onRequest`, `onResponse`) or just a request handler function.
Middleware uses standard Fetch `Request` and `Response`.

For `resolveId`, `transform`, or `onBuildComplete`, use the advanced `definePlugin({ name, setup })`
form. All hooks run in the persistent Node/Bun runtime; there is no separate compiler, debug
command, or custom middleware ABI.
