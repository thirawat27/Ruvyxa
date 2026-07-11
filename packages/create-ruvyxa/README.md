# create-ruvyxa

Create a clean Ruvyxa app with a minimal Next-style app-router starter.

## Usage

```bash
npm create ruvyxa@latest my-app
cd my-app
pnpm install
pnpm dev
```

The generated project starts with:

```text
AGENTS.md
CLAUDE.md
app/globals.css
app/layout.tsx
app/page.tsx
public/ruvyxa.png
package.json
ruvyxa.config.ts
tsconfig.json
```

The starter stays intentionally small: one page, one layout, one global stylesheet, static assets,
config, TypeScript settings, and agent instructions. Use the repository `examples/demo` when you
need examples for API routes, dynamic routes, server actions, loaders, middleware, and production
checks.

## Project Names

Project names must be valid directory names for the target operating system. On Windows, reserved
device names such as `con`, `prn`, and `aux` are rejected, and names cannot end with unsafe trailing
characters.
