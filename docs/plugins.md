# Plugins

Plugins are extension points for code that should run inside the Ruvyxa build or request lifecycle. A normal JavaScript library such as `axios`, `zod`, or `lodash` is not a plugin by itself. Use those libraries with `import`. Use a plugin when you want Ruvyxa to call extra code at a specific lifecycle point.

There are two plugin systems:

| Type | Config location | Runtime | Best for |
|------|-----------------|---------|----------|
| Build plugin | `plugins` | Node.js during build | Transforming source, resolving imports, compiling custom file formats |
| Wasm middleware plugin | `middleware.plugins` | Wasmtime during request/response | Sandboxed auth, request guards, response mutation |

This guide focuses on build plugins. See [Wasm Middleware Plugins](#wasm-middleware-plugins) for the request/response plugin model.

## Installing Build Plugins

Install a plugin package into your app:

```bash
pnpm add ruvyxa-plugin-mdx
```

Enable it by name in `ruvyxa.config.ts`:

```ts
import { defineConfig } from "ruvyxa/config"

export default defineConfig({
  plugins: ["mdx"],
})
```

Ruvyxa resolves a short name from the project root. For `"mdx"`, it tries:

```text
mdx
ruvyxa-plugin-mdx
@ruvyxa/plugin-mdx
```

You can also use the full package name:

```ts
export default defineConfig({
  plugins: ["ruvyxa-plugin-mdx"],
})
```

Use a scoped package directly when the package is scoped:

```ts
export default defineConfig({
  plugins: ["@acme/ruvyxa-plugin-mdx"],
})
```

## Using Inline Plugins

For small project-local behavior, write the plugin inline:

```ts
import { defineConfig, plugin } from "ruvyxa/config"

export default defineConfig({
  plugins: [
    plugin("replace-text", (code, id) => {
      if (!id.endsWith("page.tsx")) return null
      return code.replace("Hello", "สวัสดี")
    }),
  ],
})
```

The function form is shorthand for a `transform` hook. It receives:

| Argument | Meaning |
|----------|---------|
| `code` | Current source code |
| `id` | Absolute file path or module id |
| `ctx` | Plugin context such as `environment`, `root`, and `id` |

Return `null` or `undefined` when the plugin does not handle a file.

## Using Plugin Options

Use object form when you need hook ordering, timeouts, or import resolution:

```ts
import { defineConfig, plugin } from "ruvyxa/config"

export default defineConfig({
  plugins: [
    plugin("banner", {
      enforce: "pre",
      timeoutMs: 5000,
      transform(code, id, ctx) {
        if (ctx.environment !== "client" || !id.endsWith(".tsx")) {
          return null
        }

        return `/* built by Ruvyxa */\n${code}`
      },
    }),
  ],
})
```

Plugins run in this order:

1. `enforce: "pre"`
2. normal plugins
3. `enforce: "post"`

If a hook throws or exceeds `timeoutMs`, Ruvyxa reports `RUV1703` with the plugin name and hook name.

## Plugin Contract

A build plugin is an object with this shape:

```ts
export interface RuvyxaPlugin {
  name: string
  enforce?: "pre" | "post"
  timeoutMs?: number
  resolveId?(
    id: string,
    importer?: string,
    ctx?: PluginContext,
  ): string | null | undefined | Promise<string | null | undefined>
  transform?(
    code: string,
    id: string,
    ctx: PluginContext,
  ): string | { code: string; map?: unknown } | null | undefined | Promise<string | { code: string; map?: unknown } | null | undefined>
}
```

The plugin context is:

```ts
export interface PluginContext {
  environment: "client" | "server" | "edge" | "worker" | "shared"
  root?: string
  id?: string
}
```

Use `transform` to modify source:

```ts
export default {
  name: "replace-text",
  transform(code, id) {
    if (!id.endsWith(".tsx")) return null
    return code.replace("Hello", "Hi")
  },
}
```

Use `resolveId` to redirect imports:

```ts
export default {
  name: "alias-ui",
  resolveId(id) {
    if (id === "$ui/button") {
      return "./src/components/button.tsx"
    }

    return null
  },
}
```

## Creating A Plugin Package

The simplest plugin package is a normal ESM npm package.

```text
ruvyxa-plugin-banner/
├── package.json
├── README.md
├── tsconfig.json
└── src/
    └── index.ts
```

Use the `ruvyxa-plugin-*` naming convention when you want users to enable the plugin by short name:

```ts
export default defineConfig({
  plugins: ["banner"],
})
```

### package.json

```json
{
  "name": "ruvyxa-plugin-banner",
  "version": "1.0.0",
  "description": "Adds a build banner to Ruvyxa client modules.",
  "license": "MIT",
  "type": "module",
  "main": "./dist/index.js",
  "types": "./dist/index.d.ts",
  "files": ["dist", "README.md"],
  "keywords": ["ruvyxa", "ruvyxa-plugin"],
  "peerDependencies": {
    "ruvyxa": "^1.0.0"
  },
  "devDependencies": {
    "typescript": "^6.0.0",
    "ruvyxa": "^1.0.0"
  },
  "scripts": {
    "build": "tsc -p tsconfig.json",
    "check": "tsc -p tsconfig.json --noEmit"
  }
}
```

Use `peerDependencies` for `ruvyxa` so the app controls its framework version.

### tsconfig.json

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "declaration": true,
    "declarationMap": true,
    "sourceMap": true,
    "outDir": "dist",
    "strict": true,
    "skipLibCheck": true
  },
  "include": ["src"]
}
```

### src/index.ts

```ts
import type { RuvyxaPlugin } from "ruvyxa"

export interface BannerOptions {
  text?: string
  include?: RegExp
}

export function bannerPlugin(options: BannerOptions = {}): RuvyxaPlugin {
  const text = options.text ?? "built by Ruvyxa"
  const include = options.include ?? /\.[cm]?[jt]sx?$/

  return {
    name: "banner",
    transform(code, id) {
      if (!include.test(id)) return null
      return `/* ${text} */\n${code}`
    },
  }
}

export default bannerPlugin()
```

With this default export, users can write:

```ts
export default defineConfig({
  plugins: ["banner"],
})
```

If they want options, they can import the factory:

```ts
import { defineConfig } from "ruvyxa/config"
import { bannerPlugin } from "ruvyxa-plugin-banner"

export default defineConfig({
  plugins: [
    bannerPlugin({
      text: "internal build",
      include: /\.tsx$/,
    }),
  ],
})
```

## Supported Exports

Ruvyxa accepts any of these exports from a plugin package:

```ts
export default {
  name: "my-plugin",
  transform(code) {
    return code
  },
}
```

```ts
export const plugin = {
  name: "my-plugin",
  transform(code) {
    return code
  },
}
```

```ts
export const plugins = [
  {
    name: "first",
    transform(code) {
      return code
    },
  },
  {
    name: "second",
    transform(code) {
      return code
    },
  },
]
```

For CommonJS interop, Ruvyxa also checks `default.plugin` and `default.plugins` after importing the package.

## Project-Local Plugins

For a plugin used by one app, keep it inside the project:

```text
my-app/
├── plugins/
│   └── banner.ts
├── app/
└── ruvyxa.config.ts
```

```ts
// plugins/banner.ts
import { plugin } from "ruvyxa/config"

export default plugin("banner", (code, id) => {
  if (!id.endsWith(".tsx")) return null
  return `/* local app build */\n${code}`
})
```

```ts
// ruvyxa.config.ts
import { defineConfig } from "ruvyxa/config"
import banner from "./plugins/banner"

export default defineConfig({
  plugins: [banner],
})
```

Use local plugins while experimenting. Package the plugin only when it should be reused across projects.

## Testing A Plugin

Start with a small fixture app:

```text
fixtures/basic/
├── app/
│   └── page.tsx
├── package.json
├── ruvyxa.config.ts
└── tsconfig.json
```

Example fixture config:

```ts
import { defineConfig } from "ruvyxa/config"
import banner from "../../dist/index.js"

export default defineConfig({
  build: {
    minify: false,
  },
  plugins: [banner],
})
```

A simple smoke script can build the fixture and inspect output:

```js
import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { spawnSync } from "node:child_process"

const result = spawnSync("npx", ["ruvyxa", "build", "--root", "fixtures/basic"], {
  stdio: "inherit",
  shell: process.platform === "win32",
})

assert.equal(result.status, 0)

const manifest = await readFile("fixtures/basic/.ruvyxa/client/manifest.json", "utf8")
assert.match(manifest, /banner/)
```

Recommended checks before publishing:

```bash
pnpm run check
pnpm run build
npm pack --dry-run
```

## Publishing Checklist

Before publishing a plugin package:

- Use an ESM package (`"type": "module"`).
- Export a default plugin, `plugin`, or `plugins`.
- Put generated files in `dist/` and include them in `files`.
- Keep `ruvyxa` in `peerDependencies`.
- Document the shortest user config, usually `plugins: ["name"]`.
- Document any options and defaults.
- Add a fixture build test.
- Avoid reading secrets or environment variables unless users explicitly configure them.
- Keep network calls inside plugins optional and timeout-bound.

## Troubleshooting

### Plugin package not found

If this config fails:

```ts
plugins: ["mdx"]
```

Ruvyxa tried:

```text
mdx
ruvyxa-plugin-mdx
@ruvyxa/plugin-mdx
```

Install one of those packages or use the full package name:

```ts
plugins: ["@acme/ruvyxa-plugin-mdx"]
```

### Package loads but no plugin is found

Make sure the package exports one of:

```ts
export default pluginObject
export const plugin = pluginObject
export const plugins = [pluginObject]
```

### Hook fails during build

Ruvyxa reports `RUV1703` with the plugin name and hook:

```text
RUV1703 Plugin 'banner' transform hook failed: ...
```

Check the hook implementation, make sure async work resolves, and set `timeoutMs` when the plugin performs heavier work.

### Plain libraries are not plugins

Install ordinary libraries and import them inside app code or inside a plugin:

```ts
import axios from "axios"

export default {
  name: "api-banner",
  async transform(code) {
    const response = await axios.get("https://example.com/banner")
    return `${response.data.banner}\n${code}`
  },
}
```

`axios` itself is not a Ruvyxa plugin. The object above is the plugin because it exposes the `name` and `transform` hook that Ruvyxa can call.

## Wasm Middleware Plugins

Wasm middleware plugins are separate from build plugins. They run during request or response handling and are configured under `middleware.plugins`:

```ts
import { defineConfig } from "ruvyxa/config"

export default defineConfig({
  middleware: {
    plugins: [
      {
        name: "auth-guard",
        path: "plugins/auth-guard.wasm",
        phase: "request",
        routes: ["/api/*", "/users/:id"],
        permissions: {
          env: ["AUTH_SECRET"],
          fsRead: ["./policies"],
          timeoutMs: 5000,
          maxMemoryBytes: 67108864,
        },
      },
    ],
  },
})
```

Use Wasm plugins when you need sandboxed request/response behavior. They can be written in any language that compiles to WebAssembly and exports the Ruvyxa Wasm plugin interface, commonly Rust, TinyGo, Zig, C/C++, or AssemblyScript.

Route filters support exact paths, named parameters such as `/users/:id`, and catch-all wildcards such as `/api/*`. If `routes` is omitted, the plugin runs for every route in its phase.

`permissions.fsRead` grants read-only WASI preopened directories. Relative entries are resolved from the project root and are visible to the guest by directory name. For example, `fsRead: ["./policies"]` lets the plugin read from the guest preopen named `policies`. Environment access is limited to names listed in `permissions.env`. Network permissions are fail-closed for now: setting `permissions.net` reports an error instead of silently granting access.

Use build plugins when you need source transforms or import resolution during `ruvyxa build`.
