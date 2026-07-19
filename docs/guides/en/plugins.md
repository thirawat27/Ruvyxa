# Plugins

Ruvyxa plugins are ordinary application modules written in TypeScript.

Create a starter:

```bash
npx ruvyxa plugin new auth
```

The command creates `plugins/auth.ts`:

```ts
import { definePlugin } from 'ruvyxa/config'

export default definePlugin({
  name: 'auth',
  setup({ addMiddleware }) {
    addMiddleware({
      routes: ['/api/*'],
      onRequest(request) {
        return request.headers.has('authorization')
          ? undefined
          : new Response('Unauthorized', { status: 401 })
      },
    })
  },
})
```

Import it from `ruvyxa.config.ts`:

```ts
import auth from './plugins/auth'
import { config } from 'ruvyxa/config'

export default config({ plugins: [auth] })
```

Use `resolveId`, `transform`, and `onBuildComplete` in the same `setup` function. Middleware uses
standard Fetch `Request` and `Response`; build hooks run in the persistent Node/Bun runtime. There
there is no separate compiler, debug command, or custom middleware ABI.
