# ruvyxa-plugin-**PLUGIN_NAME**

A TypeScript plugin for Ruvyxa.

## Start

```bash
npm install
npm test
```

Edit `src/index.ts`. The generated example adds `x-__PLUGIN_NAME__: active` to every response using
the concise `headers` declaration. Add `http`, `build`, `dev`, `diagnostics`, or `native` sections
only when the plugin needs them; use `register()` for advanced composition or repeated hooks.

## Test

`test/plugin.test.mjs` drives the plugin through `createPluginHarness` from `ruvyxa/plugin-harness`.
The harness runs `register()` against recording sockets and exposes the same entry points the server
does — `request()`, `respond()`, `route()`, `fileChange()`, and the `build` hooks — so behaviour is
asserted as a unit instead of by booting a server.

```ts
const harness = await createPluginHarness(plugin)
const response = await harness.respond(new Response('ok'), '/api/items')
assert.equal(response.headers.get('x-__PLUGIN_NAME__'), 'active')
```

Route-pattern semantics match the server's: `*` matches everything, a trailing `*` matches by
prefix, and anything else matches exactly. Pass `{ environment: 'development' }` as the second
argument to exercise a development-only branch.

## Use in an app

```ts
import { config } from 'ruvyxa/config'
import __PLUGIN_IDENTIFIER__ from 'ruvyxa-plugin-__PLUGIN_NAME__'

export default config({ plugins: [__PLUGIN_IDENTIFIER__] })
```

See the Ruvyxa plugin guide for concise sections, advanced sockets, and complete examples.

## Publish

```bash
npm publish
```
