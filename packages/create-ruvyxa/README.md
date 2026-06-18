# create-ruvyxa

Create a clean Ruvyxa app with a small app-router style starter.

## Usage

```bash
npm create ruvyxa@latest my-app
cd my-app
pnpm install
pnpm dev
```

The generated project starts with:

```text
app/global.css
app/layout.tsx
app/api/health/route.ts
app/page.tsx
public/ruvyxa.png
.env.example
AGENTS.md
CLAUDE.md
package.json
ruvyxa.config.ts
tsconfig.json
```

The starter includes production-minded defaults, a health endpoint, environment documentation, and agent instructions without adding demo-only pages. Use the repository `examples/basic-app` when you need examples for dynamic routes, server actions, and loaders.

## Project Names

Project names must be valid directory names for the target operating system. On Windows, reserved device names such as `con`, `prn`, and `aux` are rejected, and names cannot end with unsafe trailing characters.
