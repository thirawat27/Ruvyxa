# CLI Commands

## ตารางคำสั่ง

| คำสั่ง                          | หน้าที่                                           |
| ------------------------------- | ------------------------------------------------- |
| `npm run dev`                   | Development server + HMR + file watching          |
| `npm run build`                 | Build production output                           |
| `npm run start`                 | Serve production build                            |
| `npm run typecheck`             | รัน `tsc --noEmit`                                |
| `npm run check`                 | รัน production readiness checks                   |
| `npx ruvyxa preview`            | ดู production build ในเครื่อง                     |
| `npx ruvyxa routes`             | แสดงตาราง routes + rendering strategy             |
| `npx ruvyxa analyze`            | ตรวจสอบ routes, imports, server/client boundaries |
| `npx ruvyxa doctor`             | ตรวจสอบ tools, dependencies, environment          |
| `npx ruvyxa trace /blog/[slug]` | ดู manifest entry ของ route                       |
| `npx ruvyxa bench`              | Benchmark discovery, analysis, production build   |
| `npx ruvyxa test:parity`        | เปรียบเทียบ dev/prod routes + smoke-render        |
| `npx ruvyxa parity`             | alias ของ `test:parity`                           |
| `npx ruvyxa clean`              | ลบ `.ruvyxa/`                                     |
| `npx ruvyxa plugin new <name>`  | สร้าง plugin starter                              |

## Common Options

| Option      | คำสั่ง                    | คำอธิบาย                        |
| ----------- | ------------------------- | ------------------------------- |
| `--root`    | ทั้งหมด                   | Project root (default: `.`)     |
| `--host`    | `dev`, `start`, `preview` | Bind host (override config)     |
| `--port`    | `dev`, `start`, `preview` | Bind port (override config)     |
| `--target`  | `build`                   | `node`, `bun`, `edge`, `static` |
| `--samples` | `bench`                   | จำนวนรอบ (default: 3)           |
| `--json`    | `bench`                   | Output เป็น JSON                |

---

## คำอธิบายแต่ละคำสั่ง

### `dev`

```bash
npx ruvyxa dev
npx ruvyxa dev --root ./my-app
npx ruvyxa dev --host 0.0.0.0 --port 8080
```

เริ่ม dev server พร้อม:

- HMR ผ่าน WebSocket — style/component อัปเดตไม่ต้อง reload ทั้งหน้า
- File watcher — detect การเปลี่ยนแปลง routes
- Render cache: 1024 entries, TTL 5 นาที
- Error overlay (`debug.overlay`)
- ตรวจจับ port conflict — auto-scan 100 ports ถัดไป

### `build`

```bash
npx ruvyxa build
npx ruvyxa build --target node      # default
npx ruvyxa build --target bun       # bun runtime
npx ruvyxa build --target static    # static output
npx ruvyxa build --target edge      # edge runtime
npx ruvyxa build --adapter vercel   # รัน deploy adapter โดยไม่ต้องแก้ config
npx ruvyxa build --runtime bun      # รัน build workers ด้วย Bun
```

`--runtime <node|bun>` ใช้ได้กับ `dev`, `start`, `preview`, `build`, `check`, `routes`, `analyze`,
`doctor`, `clean` และ `test:parity` โดย override ตัวแปรแวดล้อม `RUVYXA_RUNTIME` และ `config.runtime`
— สลับ JavaScript runtime ได้โดยไม่ต้องแก้ config

Pipeline:

1. ค้นพบ routes
2. ตรวจสอบ routes, imports, server/client boundaries
3. รวบรวม CSS styles
4. Optimize images (PNG/JPEG → WebP)
5. Bundle client code (minify, tree-shake, split)
6. Pre-render SSG / ISR / PPR / CSR routes
7. เขียน output ไปที่ `.ruvyxa/`

**Output structure:**

```text
.ruvyxa/
├── server/
│   ├── app/         # Compiled route source (จาก app/)
│   ├── components/  # จาก project components/
│   └── server/      # จาก project server/
├── client/         # BLAKE3-hashed client bundles + manifest.json
├── assets/         # Public assets + WebP images
├── prerender/      # Pre-rendered HTML pages + manifest.json
├── manifest.json   # Route manifest
└── build.json      # Metadata + timing
```

`build.json` บันทึกเวลาแต่ละ phase (discovery, validation, client bundling, prerender, adapter)
เป็นมิลลิวินาที

### `check`

```bash
npx ruvyxa check
```

รันตามลำดับ:

1. TypeScript type check (`tsc --noEmit`; ข้ามถ้าไม่มี `tsconfig.json`)
2. Parity check: build production output, เปรียบเทียบ dev/prod routes, smoke-render ทุกหน้า

ใช้เป็น deploy readiness signal

### `start` / `preview`

```bash
npx ruvyxa start
npx ruvyxa preview      # alias
```

Serve production build จาก `.ruvyxa/` ด้วย runtime semantics เดียวกับ `dev`

### `routes`

```bash
npx ruvyxa routes
```

แสดงตาราง routes พร้อม rendering strategy ที่ detect:

```text
Route                    Strategy
/                        ssg
/about                   ssg
/blog/[slug]             ssr
/api/health              api
```

### `analyze`

```bash
npx ruvyxa analyze
```

ตรวจสอบแต่ละ route (รวม imports + layouts):

- Missing default export (page.tsx)
- `"server-only"` imports ใน client code
- Private `process.env.*` access ใน client graph
- `server/` dir imports ใน client graph
- `"client-only"` imports ใน server code

Route conflict detection รันตอน route discovery (ก่อน analyze) Config validation รันตอน config
loading (ก่อน route discovery)

### `doctor`

```bash
npx ruvyxa doctor
```

รายงาน:

- Ruvyxa CLI version
- Node.js, Rust (`rustc`, `cargo`) และ Bun versions
- Package manager
- Project structure (package.json, tsconfig.json, config, app dir)
- Dependency status
- Configuration validity
- จำนวน routes (total, page, API)

### `trace`

```bash
npx ruvyxa trace /blog/hello-world
npx ruvyxa trace /blog/[slug]
```

ดู manifest entry — path, pattern, rendering strategy, layout chain, module dependencies

### `bench`

```bash
npx ruvyxa bench
npx ruvyxa bench --samples 5 --json
```

วัดประสิทธิภาพ:

- Route discovery
- Route analysis
- Production build (รวม prerender)

### `test:parity`

```bash
npx ruvyxa test:parity
npx ruvyxa parity            # alias
```

เปรียบเทียบ route list ระหว่าง dev และ production + smoke-render ทุก page route

### `clean`

```bash
npx ruvyxa clean
```

ลบ `.ruvyxa/` ทั้งหมด

### `plugin new`

```bash
npx ruvyxa plugin new request-logger
```

สร้าง plugin package ที่ `request-logger/` ตรงๆ (ชื่อโฟลเดอร์ = ชื่อ plugin ไม่มีชั้น `plugins/`
ครอบ) พร้อม `src/index.ts`, `package.json`, `tsconfig.json`, `README.md` ใส่ `--dir <path>` เฉพาะถ้า
ต้องการเลือกตำแหน่งอื่น (relative จาก `--root` และห้ามมี `..`) scaffold ใช้ได้ทั้ง Node.js และ Bun —
build ด้วย `npm run build` (หรือ `bun run build`) แล้ว publish ด้วย package manager ที่ใช้ import
package entry จาก `ruvyxa.config.ts` และลงทะเบียนใน `config({ plugins: [...] })` ดู workflow
เต็มได้ที่ [Plugins](plugins.md)

## Environment Variables

| Variable                   | หน้าที่                                                            | ค่าเริ่มต้น             |
| -------------------------- | ------------------------------------------------------------------ | ----------------------- |
| `RUVYXA_RENDER_CACHE_SIZE` | Render cache size; `0` = ปิด cache, max 16,384                     | 1024 (dev), 512 (prod)  |
| `RUVYXA_BUILD_CACHE_DIR`   | เปลี่ยนตำแหน่ง build cache                                         | `.ruvyxa/cache/bundler` |
| `RUVYXA_WORKER_TIMEOUT_MS` | timeout request/API stream (1–2,147,483,647 ms)                    | 30,000 ms               |
| `RUVYXA_MEMORY_LIMIT_MB`   | จุดเริ่มลด worker cache ตาม memory (JS worker เท่านั้น)            | 512 MiB                 |
| `RUVYXA_CACHE_MAX_ENTRIES` | จำนวน compiled bundle/module สูงสุดต่อ worker (JS worker เท่านั้น) | 256                     |
