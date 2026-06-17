# Ruvyxa Framework Specification

> เอกสารออกแบบสำหรับสั่ง AI / ทีมพัฒนาให้สร้าง **JavaScript/TypeScript Full‑stack Framework + Rust Dev Server + Rust Build Tool** ที่เร็วมาก เขียนง่ายระดับ Next.js เรียนวันเดียวเริ่มทำงานได้ และทำให้ `dev` กับ `production` ตรงกันมากที่สุด

---

## 0. สรุปชื่อและสถานะการค้นหา

**ชื่อหลักที่แนะนำ:** `Ruvyxa`

**อ่านว่า:** รู‑วิก‑ซา / Roo-vik-sa

**เหตุผลของชื่อ:**

- สั้น 6 ตัวอักษร จำง่าย พิมพ์ง่าย
- มีตัว `R` สื่อถึง Rust runtime/build tool
- มีเสียง `vx` ที่สื่อถึง velocity + developer experience
- ไม่ผูกกับคำสามัญ เช่น app, web, fast, vite, next จึงทำ branding ได้ง่ายกว่า
- จากการค้น web แบบ public ณ วันที่ 5 มิถุนายน 2026 ไม่พบผลลัพธ์ตรงที่เป็น JavaScript/TypeScript framework, npm package, Rust crate หรือ GitHub project หลักชื่อ `Ruvyxa`

**ข้อควรทำก่อนเปิดตัวจริง:**

- จอง npm scope: `@ruvyxa/*`
- จอง crates.io prefix: `ruvyxa-*`
- จอง GitHub org: `ruvyxa`
- จองโดเมนอย่างน้อย: `ruvyxa.dev`, `ruvyxa.com`, `ruvyxa.rs`
- เช็ก trademark อย่างเป็นทางการในประเทศเป้าหมาย เช่น US/EU/Thailand ก่อนประกาศ public

> หมายเหตุ: ไม่มีการค้นใดรับประกันว่า “ไม่มีใครใช้ 100% ทั่วโลก” ได้ เพราะต้องตรวจ domain, trademark, company registry, private repos และ package registry แบบ official เพิ่มเติม แต่ชื่อ `Ruvyxa` เป็นตัวเลือกที่ conflict ต่ำมากจากการค้น public ที่ทำได้ตอนนี้

---

## 1. Vision

Ruvyxa คือ full‑stack web framework สำหรับ JavaScript/TypeScript ที่มีเป้าหมายหลัก 4 อย่าง:

1. **เร็วมาก** ทั้งตอน dev, HMR, build, SSR, deploy
2. **เขียนง่ายมาก** เรียนวันเดียวเริ่มสร้าง app ได้
3. **ฉลาดและ debug ง่าย** framework ต้องอธิบาย error, dependency graph, route graph และ runtime behavior ได้
4. **Dev/Production ตรงกันกว่าเดิม** ลดปัญหา “ตอน dev ได้ ตอน deploy พัง”

Ruvyxa ไม่ใช่แค่ framework แต่เป็น **compiler + runtime + dev server + build tool + deploy adapters** ที่ออกแบบเป็นระบบเดียวกันตั้งแต่ต้น

---

## 2. Positioning

### 2.1 One-liner

> Ruvyxa is a Rust-powered full‑stack TypeScript framework with a smart dev server, production‑accurate builds, typed server functions, file routing, and explainable debugging.

### 2.2 เปรียบเทียบเชิงแนวคิด

| Framework/Tool | จุดแข็ง | ช่องว่างที่ Ruvyxa จะโจมตี |
|---|---|---|
| Next.js | Full‑stack, ecosystem ใหญ่, App Router, React Server Components | complexity สูง, cache/debug ยาก, dev/prod mismatch บางกรณี |
| Vite | Dev server เร็ว, HMR ดี, plugin ecosystem ดี | ไม่ใช่ full‑stack framework โดยตัวเอง |
| Remix | Web fundamentals, nested routing, actions/loaders | tooling/build server ไม่ได้ถูกออกแบบเป็น Rust-first ทั้งระบบ |
| SvelteKit | syntax ง่าย, full‑stack ดี | ผูกกับ Svelte, ecosystem React น้อยกว่า |
| Astro | content/static/partial hydration ดี | full‑stack app ที่ซับซ้อนต้องพึ่ง integration เพิ่ม |
| Ruvyxa | framework + build tool + dev server Rust-first | ต้องสร้าง ecosystem เองและต้องคุม scope ให้ดี |

---

## 3. Product Principles

### 3.1 Learn in one day

คนที่รู้ TypeScript + React/JSX ควรเรียน Ruvyxa ได้ภายใน 1 วัน โดยมี mental model แค่ 6 เรื่อง:

1. `app/` คือ routes
2. `page.tsx` คือหน้า
3. `layout.tsx` คือ layout ซ้อนกันได้
4. `server.ts` คือ server-only logic
5. `action.ts` คือ mutation/form/API ที่ type-safe
6. `ruvyxa dev` กับ `ruvyxa build && ruvyxa start` ใช้ runtime core เดียวกัน

### 3.2 Fast by default

- Rust เป็น engine หลักสำหรับ dev server, graph, transforms, bundling, minify, manifest, route analysis
- Lazy transform เฉพาะไฟล์ที่ request ใน dev
- Incremental graph สำหรับ HMR และ rebuild
- Persistent cache บน disk
- Production build แยก client/server/edge graph อย่างชัดเจน

### 3.3 Smart by default

Framework ต้องทำได้มากกว่ารายงาน stack trace:

- บอกว่า error เกิดจาก route ไหน
- บอกว่า module ไหนกลายเป็น client bundle เพราะ import ผิด
- บอกว่า server-only code หลุดไป client ได้อย่างไร
- บอก cache key, loader/action call chain, waterfall, slow module
- เสนอ fix แบบ copy/paste ได้

### 3.4 Dev equals production

ใน dev ต้องใช้ production runtime core เดียวกัน แต่เปิด debug layer เพิ่ม:

```txt
Dev Runtime = Production Runtime Core + HMR + Overlay + Source Maps + Debug Trace
Prod Runtime = Production Runtime Core + Optimized Graph + Minified Assets
```

ห้ามมี behavior สำคัญที่ต่างกัน เช่น routing, middleware order, server action protocol, env loading, cache semantics

---

## 4. Target Users

### 4.1 Primary

- indie dev ที่ชอบ Next.js แต่รู้สึกว่า config/cache/debug ยาก
- startup ที่ต้องการ full‑stack TS เร็วและ deploy ง่าย
- team ที่อยากได้ DX ดี แต่ต้องการ build/dev tool เร็วระดับ Rust

### 4.2 Secondary

- agency ที่ต้องสร้างหลายเว็บเร็ว ๆ
- internal tools
- SaaS dashboards
- content + app hybrid

---

## 5. Non-goals สำหรับช่วงแรก

เพื่อไม่ให้โปรเจกต์ใหญ่เกินไปใน MVP:

- ยังไม่สร้าง UI library เอง
- ยังไม่ทำ mobile framework
- ยังไม่ทำ ORM เอง
- ยังไม่สร้าง JS runtime เองแทน Node/Bun/Deno
- ยังไม่รองรับทุก framework UI ตั้งแต่วันแรก
- ยังไม่พยายามแทน webpack/vite ecosystem ทั้งหมดทันที

---

## 6. Core Architecture

```txt
┌───────────────────────────────────────────────────────────┐
│                         User App                          │
│  app/ routes | server actions | components | styles | db   │
└────────────────────────────┬──────────────────────────────┘
                             │
┌────────────────────────────▼──────────────────────────────┐
│                    Ruvyxa TypeScript API                  │
│  routing | data | actions | cache | config | adapters      │
└────────────────────────────┬──────────────────────────────┘
                             │
┌────────────────────────────▼──────────────────────────────┐
│                    Ruvyxa Rust Core                       │
│  parser | graph | dev server | bundler | manifest | HMR    │
│  diagnostics | source maps | optimizer | asset pipeline    │
└────────────────────────────┬──────────────────────────────┘
                             │
┌────────────────────────────▼──────────────────────────────┐
│                     Runtime Targets                       │
│ Node | Bun | Deno | Edge | Serverless | Static/Hybrid      │
└───────────────────────────────────────────────────────────┘
```

---

## 7. Repository Structure

```txt
ruvyxa/
├─ crates/
│  ├─ ruvyxa_cli/              # CLI entry: ruvyxa dev/build/start/doctor
│  ├─ ruvyxa_dev_server/       # HTTP server, HMR WS, middleware pipeline
│  ├─ ruvyxa_graph/            # dependency graph, route graph, incremental cache
│  ├─ ruvyxa_parser/           # JS/TS/TSX parser wrapper
│  ├─ ruvyxa_transform/        # transforms, JSX, server/client split
│  ├─ ruvyxa_bundler/          # bundling, chunking, tree shaking
│  ├─ ruvyxa_css/              # CSS pipeline, modules, nesting, minify
│  ├─ ruvyxa_diagnostics/      # error messages, overlay data, source maps
│  ├─ ruvyxa_runtime_manifest/ # manifest generation and validation
│  └─ ruvyxa_adapters/         # node/edge/serverless/static adapters
│
├─ packages/
│  ├─ ruvyxa/                  # public CLI npm package wrapper
│  ├─ @ruvyxa/core/            # public TS framework API
│  ├─ @ruvyxa/react/           # React/JSX renderer integration v1
│  ├─ @ruvyxa/server/          # runtime server helpers
│  ├─ @ruvyxa/client/          # client hydration/runtime
│  ├─ @ruvyxa/dev-overlay/     # browser overlay UI
│  ├─ @ruvyxa/plugin/          # plugin authoring API
│  └─ create-ruvyxa/           # starter generator
│
├─ examples/
│  ├─ basic-app/
│  ├─ blog-mdx/
│  ├─ auth-dashboard/
│  ├─ ecommerce/
│  └─ edge-api/
│
├─ templates/
│  ├─ minimal/
│  ├─ fullstack/
│  ├─ dashboard/
│  └─ docs-blog/
│
├─ docs/
│  ├─ getting-started.md
│  ├─ routing.md
│  ├─ data.md
│  ├─ actions.md
│  ├─ deployment.md
│  ├─ debugging.md
│  └─ plugin-api.md
│
└─ benches/
   ├─ cold-start/
   ├─ hmr/
   ├─ build/
   └─ ssr/
```

---

## 8. User App Structure

```txt
my-app/
├─ app/
│  ├─ layout.tsx
│  ├─ page.tsx
│  ├─ loading.tsx
│  ├─ error.tsx
│  ├─ about/
│  │  └─ page.tsx
│  ├─ blog/
│  │  ├─ page.tsx
│  │  └─ [slug]/
│  │     ├─ page.tsx
│  │     └─ server.ts
│  └─ api/
│     └─ health/
│        └─ route.ts
│
├─ components/
│  ├─ Button.tsx
│  └─ Nav.tsx
│
├─ server/
│  ├─ db.ts
│  ├─ auth.ts
│  └─ env.ts
│
├─ public/
│  └─ favicon.svg
│
├─ ruvyxa.config.ts
├─ package.json
└─ tsconfig.json
```

---

## 9. Developer Experience: ตัวอย่างโค้ด

### 9.1 หน้าแรก

```tsx
// app/page.tsx
export default function Home() {
  return (
    <main>
      <h1>Hello Ruvyxa</h1>
      <p>Full‑stack TypeScript, powered by Rust.</p>
    </main>
  )
}
```

### 9.2 Layout

```tsx
// app/layout.tsx
import "./global.css"

export const meta = {
  title: "My Ruvyxa App",
  description: "Fast full‑stack app",
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
```

### 9.3 Dynamic Route + Server Loader

```tsx
// app/blog/[slug]/server.ts
import { loader } from "ruvyxa/server"
import { db } from "~/server/db"

export const getPost = loader(async ({ params }) => {
  const post = await db.post.findUnique({ where: { slug: params.slug } })

  if (!post) {
    throw new Response("Post not found", { status: 404 })
  }

  return post
})
```

```tsx
// app/blog/[slug]/page.tsx
import { getPost } from "./server"

export default async function BlogPost() {
  const post = await getPost()

  return (
    <article>
      <h1>{post.title}</h1>
      <div dangerouslySetInnerHTML={{ __html: post.html }} />
    </article>
  )
}
```

### 9.4 Server Action

```tsx
// app/todos/action.ts
import { action } from "ruvyxa/server"
import { z } from "zod"
import { db } from "~/server/db"

export const createTodo = action
  .input(z.object({ title: z.string().min(1) }))
  .handler(async ({ input, user }) => {
    if (!user) throw new Response("Unauthorized", { status: 401 })

    return db.todo.create({
      data: {
        title: input.title,
        userId: user.id,
      },
    })
  })
```

```tsx
// app/todos/page.tsx
import { createTodo } from "./action"

export default function TodosPage() {
  return (
    <form action={createTodo}>
      <input name="title" placeholder="What needs to be done?" />
      <button>Add</button>
    </form>
  )
}
```

### 9.5 API Route

```ts
// app/api/health/route.ts
export function GET() {
  return Response.json({ ok: true, framework: "Ruvyxa" })
}
```

---

## 10. Routing Design

### 10.1 File conventions

| File | Meaning |
|---|---|
| `page.tsx` | UI page |
| `layout.tsx` | nested layout |
| `loading.tsx` | loading UI |
| `error.tsx` | route error boundary |
| `not-found.tsx` | 404 boundary |
| `server.ts` | server-only loader/helper |
| `action.ts` | typed mutation/action |
| `route.ts` | HTTP route handler |
| `middleware.ts` | route middleware |
| `client.tsx` | explicit client island |

### 10.2 Dynamic route syntax

```txt
app/users/[id]/page.tsx          -> /users/:id
app/docs/[...slug]/page.tsx      -> /docs/*slug
app/shop/[[category]]/page.tsx   -> optional category
app/(marketing)/page.tsx         -> route group, no URL segment
app/@modal/product/[id]/page.tsx -> parallel/intercepted route v2
```

### 10.3 Route manifest

Rust core ต้องสร้าง route manifest ตอน dev/build:

```json
{
  "routes": [
    {
      "id": "app/blog/[slug]/page",
      "path": "/blog/:slug",
      "type": "page",
      "layoutChain": ["app/layout", "app/blog/layout"],
      "serverModules": ["app/blog/[slug]/server"],
      "clientModules": ["components/LikeButton"],
      "runtime": "node"
    }
  ]
}
```

---

## 11. Rendering Model

Ruvyxa v1 ควรใช้ **React-compatible rendering** ก่อน เพื่อให้ผู้ใช้ Next.js ย้ายมาได้ง่าย และลด scope ในการสร้าง UI runtime เอง

### 11.1 Rendering modes

| Mode | Use case | Output |
|---|---|---|
| Static | docs, marketing, blog | HTML + assets |
| SSR | dashboard, auth pages | HTML streaming |
| ISR-like revalidate | product/content pages | cached HTML/data |
| SPA island | interactive widget | client chunk |
| API-only | backend endpoints | Response |
| Edge | latency-sensitive routes | edge bundle |

### 11.2 Route-level config

```ts
// app/products/[id]/page.tsx
export const runtime = "node"       // node | edge | static
export const revalidate = 60        // seconds | false
export const cache = "route"        // none | data | route
```

---

## 12. Data Layer

Ruvyxa ไม่ควรสร้าง ORM เอง แต่ควรทำ data orchestration ที่ดี:

### 12.1 Loader

```ts
export const getUser = loader(async ({ params, request, cache }) => {
  return cache.key(`user:${params.id}`).ttl("5m").get(async () => {
    return db.user.findUnique({ where: { id: params.id } })
  })
})
```

### 12.2 Action

```ts
export const updateUser = action
  .input(UserSchema)
  .handler(async ({ input, user, invalidate }) => {
    const updated = await db.user.update({ where: { id: user.id }, data: input })
    invalidate(`user:${user.id}`)
    return updated
  })
```

### 12.3 Cache debug

ใน dev overlay ต้องแสดง:

```txt
Route: /users/123
Loader: getUser
Cache key: user:123
Cache status: HIT
TTL: 4m 12s remaining
Invalidated by: updateUser at app/users/action.ts:12
```

---

## 13. Rust Build Tool Design

### 13.1 Goals

- ทำหน้าที่แทน bundler + transformer + dev server
- ใช้ Rust เป็น core เพื่อ latency ต่ำ
- support TypeScript/TSX/JSX/CSS/JSON/WASM/Workers
- plugin system ที่รองรับ JS plugins และ Rust plugins

### 13.2 Build pipeline

```txt
1. Read config
2. Discover routes
3. Build dependency graph
4. Split graph: server / client / edge / worker
5. Transform TS/TSX/JSX
6. CSS processing
7. Tree shaking
8. Chunking
9. Minify
10. Emit assets
11. Emit server bundle
12. Emit route manifest
13. Validate dev/prod parity
```

### 13.3 Internal graph node

```rust
pub struct ModuleNode {
    pub id: ModuleId,
    pub path: PathBuf,
    pub kind: ModuleKind,
    pub imports: Vec<ImportEdge>,
    pub importers: Vec<ModuleId>,
    pub side_effects: bool,
    pub environment: Environment,
    pub hash: ContentHash,
    pub transform_cache_key: String,
}

pub enum Environment {
    Client,
    Server,
    Edge,
    Worker,
    Shared,
}
```

### 13.4 Incremental cache

Cache key ต้องรวม:

- file content hash
- transform options
- env target
- tsconfig hash
- ruvyxa config hash
- package lock hash
- plugin versions
- Rust binary version

---

## 14. Dev Server Design

### 14.1 Commands

```bash
npx create-ruvyxa@latest my-app
cd my-app
npm run dev
```

```bash
ruvyxa dev
ruvyxa build
ruvyxa start
ruvyxa preview
ruvyxa doctor
ruvyxa trace /dashboard
ruvyxa routes
ruvyxa analyze
```

### 14.2 Dev server components

```txt
┌────────────────────┐
│ HTTP Server         │
├────────────────────┤
│ Middleware Pipeline │
├────────────────────┤
│ Route Resolver      │
├────────────────────┤
│ Module Transformer  │
├────────────────────┤
│ HMR Graph           │
├────────────────────┤
│ WebSocket HMR       │
├────────────────────┤
│ Error Overlay API   │
├────────────────────┤
│ Runtime Trace API   │
└────────────────────┘
```

### 14.3 HMR strategy

- ถ้าแก้ client component -> update client boundary
- ถ้าแก้ CSS -> inject CSS patch
- ถ้าแก้ server-only file -> invalidate route server module, refresh only affected route
- ถ้าแก้ layout -> invalidate subtree
- ถ้าแก้ config/env -> restart controlled worker, preserve browser session if possible

### 14.4 Dev server target behavior

| Event | Expected behavior |
|---|---|
| แก้ component leaf | HMR ไม่ reload ทั้งหน้า |
| แก้ server loader | reload affected route พร้อม trace |
| import server module ใน client | error ทันทีพร้อม path chain |
| env var หาย | error ก่อน route render |
| route conflict | error ตอน startup พร้อม route table |

---

## 15. Smart Diagnostics

### 15.1 Error format

Error ต้องตอบ 5 คำถาม:

1. เกิดอะไรขึ้น
2. เกิดที่ไหน
3. ทำไมถึงเกิด
4. แก้ยังไง
5. กระทบ route/module ไหนบ้าง

ตัวอย่าง:

```txt
RUV1007: Server-only module imported into client bundle

Client file:
  components/UserMenu.tsx:3:1

Import chain:
  components/UserMenu.tsx
  └─ server/db.ts

Why this is a problem:
  server/db.ts uses process.env.DATABASE_URL and cannot run in the browser.

Fix:
  Move database access into app/users/server.ts and pass data as props.

Affected routes:
  /dashboard
  /users/:id
```

### 15.2 Overlay tabs

- Error
- Import chain
- Route graph
- Bundle impact
- Cache trace
- Network/server action trace
- Suggested fixes

---

## 16. Dev/Production Parity

### 16.1 Same runtime core

```txt
@ruvyxa/server/runtime-core
├─ request parsing
├─ routing
├─ middleware order
├─ loader/action protocol
├─ cache API
├─ error boundary resolution
└─ response streaming
```

ทั้ง `dev`, `start`, `preview`, adapter ทุกตัวต้องใช้ core นี้

### 16.2 Build validation

`ruvyxa build` ต้องตรวจ:

- server-only leak
- client-only leak
- missing env
- dynamic route conflict
- edge-incompatible Node APIs
- hydration risk
- oversized client chunks
- non-deterministic imports

### 16.3 Parity tests

ทุก route ต้องมี snapshot จาก dev/prod runtime:

```bash
ruvyxa test:parity
```

ผลลัพธ์:

```txt
✓ /                    dev/prod match
✓ /blog/hello          dev/prod match
✗ /dashboard           header mismatch: Set-Cookie differs
```

---

## 17. Configuration

```ts
// ruvyxa.config.ts
import { defineConfig } from "ruvyxa/config"

export default defineConfig({
  appDir: "app",
  outDir: ".ruvyxa",
  runtime: "node",
  react: true,
  typescript: {
    strict: true,
  },
  css: {
    modules: true,
    nesting: true,
  },
  server: {
    port: 3000,
    host: "localhost",
  },
  build: {
    minify: true,
    sourcemap: true,
    splitStrategy: "route",
  },
  debug: {
    overlay: true,
    traces: true,
  },
})
```

---

## 18. Plugin API

### 18.1 JS plugin

```ts
import type { RuvyxaPlugin } from "ruvyxa/plugin"

export default function mdxPlugin(): RuvyxaPlugin {
  return {
    name: "mdx",
    enforce: "pre",
    resolveId(id) {
      if (id.endsWith(".mdx")) return id
    },
    async transform(code, id, ctx) {
      if (!id.endsWith(".mdx")) return null
      return {
        code: await compileMdx(code),
        map: null,
      }
    },
  }
}
```

### 18.2 Rust plugin v2

Rust plugins ใช้สำหรับ performance-sensitive transform เช่น image, CSS, compression

---

## 19. Security Model

### 19.1 Server/client boundary

ต้องมี explicit boundary:

```ts
import "server-only"
```

```ts
import "client-only"
```

Rust graph analyzer ต้อง block leak ทันที

### 19.2 Env handling

- `RUVYXA_PUBLIC_*` ใช้บน client ได้
- env อื่นใช้ server-only เท่านั้น
- build ต้อง fail ถ้า client bundle แตะ private env

### 19.3 Action security

Server actions ต้องมี:

- CSRF protection default
- origin check
- serialized input validation
- size limit
- rate-limit hook
- audit trace ใน dev

---

## 20. Deployment Adapters

### 20.1 Adapter interface

```ts
export interface Adapter {
  name: string
  target: "node" | "edge" | "serverless" | "static"
  build(ctx: BuildContext): Promise<AdapterOutput>
}
```

### 20.2 First-party adapters

```txt
@ruvyxa/adapter-node
@ruvyxa/adapter-vercel
@ruvyxa/adapter-cloudflare
@ruvyxa/adapter-netlify
@ruvyxa/adapter-bun
@ruvyxa/adapter-static
```

---

## 21. CLI Specification

```bash
ruvyxa dev [--port 3000] [--host 0.0.0.0]
ruvyxa build [--target node|edge|static]
ruvyxa start
ruvyxa preview
ruvyxa routes
ruvyxa analyze
ruvyxa doctor
ruvyxa clean
ruvyxa trace <route>
```

### 21.1 `ruvyxa doctor`

ตรวจ:

- Node/Bun/Deno version
- package manager
- tsconfig
- dependency duplicates
- React version compatibility
- invalid route files
- env schema
- native binary compatibility

### 21.2 `ruvyxa analyze`

แสดง:

- route bundles
- largest client modules
- duplicate dependencies
- server/client split
- unused route assets
- compression estimate

---

## 22. Performance Targets

> ทั้งหมดเป็นเป้าหมาย ไม่ใช่ guarantee ต้องมี benchmark suite วัดจริง

### 22.1 Dev server

| Scenario | Target |
|---|---:|
| cold start app 20 routes | < 300 ms |
| cold start app 100 routes | < 800 ms |
| client component HMR | < 50 ms |
| CSS HMR | < 30 ms |
| server loader edit | < 120 ms |

### 22.2 Build

| Scenario | Target |
|---|---:|
| basic app | < 2 s |
| 100 route app | < 10 s |
| rebuild no-op | < 500 ms |
| route-level incremental build | < 1 s |

### 22.3 Runtime

| Scenario | Target |
|---|---:|
| SSR hello world TTFB local | < 20 ms |
| route manifest lookup | O(segments) |
| action dispatch overhead | < 2 ms excluding user code |

---

## 23. MVP Plan

### Phase 0: Research + Naming + Prototype

- จองชื่อ/package/org
- เลือก parser/transpiler strategy
- ทำ Rust CLI hello world
- ทำ dev server serve static + TS transform
- ทำ route discovery จาก `app/`

### Phase 1: Minimal Full‑stack

- `page.tsx`
- `layout.tsx`
- static routing
- dynamic routing
- SSR React
- client hydration
- CSS import
- `route.ts` API
- dev overlay basic

### Phase 2: Rust Build Tool

- dependency graph
- client/server split
- production build
- asset hashing
- route manifest
- source maps
- `ruvyxa start`

### Phase 3: Smart DX

- server/client leak detection
- route conflict diagnostics
- import chain explanation
- cache trace
- `ruvyxa doctor`
- `ruvyxa analyze`

### Phase 4: Data + Actions

- `loader()`
- `action()`
- type-safe form actions
- cache API
- invalidation
- dev/prod parity tests

### Phase 5: Ecosystem

- adapters
- MDX plugin
- image plugin
- docs site
- examples
- migration guide from Next.js

---

## 24. AI Implementation Instructions

ใช้ prompt นี้เพื่อให้ AI เริ่ม implement แบบเป็นขั้นตอน

```md
You are building Ruvyxa, a Rust-powered full-stack JavaScript/TypeScript framework.

Primary goal:
Create a production-grade monorepo containing:
1. Rust CLI and dev server
2. Rust dependency graph and route discovery
3. TypeScript framework API
4. React-compatible SSR + hydration MVP
5. File-based routing using app/
6. Production build output
7. Smart diagnostics and debug overlay

Hard requirements:
- Use Rust for CLI/dev server/build core.
- Use TypeScript for public framework API.
- Dev and production must share the same runtime semantics.
- Keep the first MVP small and testable.
- Do not build an ORM, UI library, or custom JS runtime in v1.
- Implement useful errors before adding advanced features.

Architecture:
- crates/ for Rust crates
- packages/ for TypeScript packages
- examples/ for runnable apps
- docs/ for documentation

First milestone:
Create a working app where:
- `ruvyxa dev` starts a server
- `app/page.tsx` renders HTML
- CSS import works
- changing a component updates browser via HMR or full reload
- `ruvyxa build` emits `.ruvyxa/` output
- `ruvyxa start` serves the production build

Testing:
- Unit tests for route discovery
- Unit tests for route manifest
- Integration test for dev server
- Integration test for build/start parity
- Snapshot test for diagnostics

Coding style:
- Small modules
- Explicit error types
- No silent fallback
- Good logs
- Clear README for each crate/package
```

---

## 25. AI Task Breakdown

### Task 1: Bootstrap monorepo

```md
Create the Ruvyxa monorepo with Rust workspace and pnpm workspace.
Set up crates:
- ruvyxa_cli
- ruvyxa_dev_server
- ruvyxa_graph
- ruvyxa_diagnostics
Set up packages:
- ruvyxa
- @ruvyxa/core
- @ruvyxa/react
- create-ruvyxa
Add root README, CONTRIBUTING, LICENSE, rustfmt, clippy, eslint, tsconfig.
```

### Task 2: CLI

```md
Implement `ruvyxa` CLI in Rust with commands:
- dev
- build
- start
- routes
- doctor
For now, dev starts a local HTTP server and returns a basic HTML response.
Add structured logging and error handling.
```

### Task 3: Route discovery

```md
Implement app directory route discovery.
Support:
- app/page.tsx -> /
- app/about/page.tsx -> /about
- app/blog/[slug]/page.tsx -> /blog/:slug
Generate a RouteManifest JSON.
Add tests for static, nested, dynamic, catch-all, and conflict routes.
```

### Task 4: SSR MVP

```md
Implement React-compatible SSR for page.tsx.
The dev server should load the route module, render HTML, and inject client entry.
Keep implementation simple and document limitations.
```

### Task 5: Transform pipeline

```md
Implement TS/TSX transform integration.
Support JSX, TypeScript stripping, source maps, and import rewriting for dev.
Add transform cache keyed by file hash and config hash.
```

### Task 6: HMR

```md
Implement HMR WebSocket.
Watch files under app/, components/, server/.
When a file changes, update the graph and send an HMR event.
Start with full-page reload, then add component-level HMR later.
```

### Task 7: Production build

```md
Implement `ruvyxa build`.
Output:
- .ruvyxa/server/
- .ruvyxa/client/
- .ruvyxa/manifest.json
- .ruvyxa/assets/
Implement `ruvyxa start` to serve the production output using the same runtime routing semantics.
```

### Task 8: Diagnostics

```md
Implement diagnostic system with error code, title, explanation, file span, import chain, and suggested fix.
Add diagnostics for:
- duplicate routes
- missing page default export
- server-only import inside client
- invalid route config
```

### Task 9: Data loaders and actions

```md
Implement `loader()` and `action()` API in @ruvyxa/core.
Add server protocol for action invocation.
Add validation hook and examples with zod.
Add dev trace output for loader/action calls.
```

### Task 10: Documentation

```md
Write docs for:
- Getting started
- File routing
- Layouts
- Data loaders
- Actions
- API routes
- CSS
- Debugging
- Deployment
Also create examples/basic-app and examples/auth-dashboard.
```

---

## 26. Acceptance Criteria

MVP ถือว่า usable เมื่อ:

- สร้าง app ใหม่ด้วย `npm create ruvyxa@latest`
- รัน `npm run dev` แล้วเห็นหน้าแรกภายใน browser
- เพิ่ม route ใหม่แล้วไม่ต้อง restart server
- `page.tsx`, `layout.tsx`, dynamic route ทำงาน
- API route `route.ts` ทำงาน
- CSS import ทำงาน
- build แล้ว start production ได้
- error หลักมี message ที่เข้าใจได้
- มี docs เริ่มต้นครบ
- มี benchmark เทียบ cold start/HMR/build

---

## 27. Risk Register

| Risk | Severity | Mitigation |
|---|---:|---|
| Scope ใหญ่เกินไป | สูง | v1 ใช้ React-compatible runtime ก่อน ไม่สร้าง UI runtime เอง |
| Bundler ยากมาก | สูง | เริ่มจาก dev transform + simple production bundling แล้วค่อยเพิ่ม optimizer |
| Plugin ecosystem น้อย | กลาง | ออกแบบ Vite-like plugin compatibility บางส่วนในอนาคต |
| Dev/prod parity ทำยาก | สูง | บังคับ runtime core เดียวตั้งแต่วันแรก |
| Server action security | สูง | CSRF/origin/schema/size limit default |
| Debug overlay กลายเป็นงานใหญ่ | กลาง | เริ่มจาก text overlay + JSON trace ก่อน |
| ชื่อ brand อาจชน trademark | กลาง | เช็กและจองอย่างเป็นทางการก่อน launch |

---

## 28. Recommended Tech Choices

### Rust

- CLI: `clap`
- async runtime: `tokio`
- HTTP: `axum` หรือ `hyper`
- file watch: `notify`
- JSON: `serde`
- diagnostics: custom + `miette`
- hashing: `xxhash-rust` หรือ `blake3`
- source maps: sourcemap crate

### JS/TS

- package manager: pnpm
- UI v1: React-compatible
- validation examples: zod
- test runner: vitest for TS packages
- integration tests: playwright

### Build/transform options

เลือกได้ 2 ทาง:

1. ใช้ existing Rust parser/compiler เช่น SWC หรือ Oxc เพื่อ MVP เร็วขึ้น
2. สร้าง parser/transform เองในระยะยาวเฉพาะส่วนที่ต้อง optimize

คำแนะนำ: เริ่มจาก existing compiler แล้วสร้าง Ruvyxa graph/runtime/diagnostics ให้เด่นก่อน

---

## 29. Public API Draft

```ts
import {
  defineConfig,
  loader,
  action,
  cache,
  redirect,
  notFound,
  json,
} from "ruvyxa"
```

### `loader()`

```ts
const getData = loader(async (ctx) => data)
```

### `action()`

```ts
const save = action.input(schema).handler(async (ctx) => result)
```

### `cache()`

```ts
const data = await cache("key").ttl("10m").get(fn)
```

### `redirect()`

```ts
throw redirect("/login")
```

### `notFound()`

```ts
throw notFound()
```

---

## 30. Documentation Tone

Docs ต้องใช้ภาษาสั้น ง่าย เห็นตัวอย่างก่อน theory

โครง docs:

1. Start in 60 seconds
2. Your first page
3. Add a layout
4. Add data
5. Add a form action
6. Add an API route
7. Debug an error
8. Build and deploy

ทุกหน้า docs ควรมี:

- Example
- Explanation
- Common mistakes
- Debug tips
- Link ไป API reference

---

## 31. Branding

### Tagline options

- `Ruvyxa: Full-stack TypeScript at Rust speed.`
- `Ruvyxa: The framework that explains itself.`
- `Ruvyxa: Fast dev. Honest prod.`
- `Ruvyxa: Build full‑stack apps without fighting the framework.`

### Brand keywords

- Fast
- Clear
- Smart
- Honest
- Full‑stack
- Rust-powered
- Debuggable
- Production-accurate

---

## 32. Final Recommendation

เริ่มสร้าง Ruvyxa แบบ **framework-first, bundler-second**:

1. อย่าเริ่มจากการสร้าง bundler ที่สมบูรณ์ทันที
2. เริ่มจาก route discovery + dev server + SSR + diagnostics
3. ทำให้ user เขียน app ได้ก่อน
4. ค่อยเพิ่ม build optimization, HMR ฉลาด, cache, action, deploy adapter
5. จุดขายที่ควรชนะตั้งแต่แรกคือ “debug ง่าย + dev/prod ตรงกัน” เพราะความเร็วอย่างเดียวมีคู่แข่งเยอะแล้ว

