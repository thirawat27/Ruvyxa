# CLI Commands

| คำสั่ง                          | หน้าที่                                                 |
| ------------------------------- | ------------------------------------------------------- |
| `npx ruvyxa dev`                | Dev server + HMR + file watching                        |
| `npx ruvyxa build`              | Production build → `.ruvyxa/`                           |
| `npx ruvyxa check`              | Typecheck + build + parity + smoke render               |
| `npx ruvyxa start`              | Serve production build                                  |
| `npx ruvyxa preview`            | Preview production build (alias `start`)                |
| `npx ruvyxa routes`             | พิมพ์ตาราง routes + rendering strategy                  |
| `npx ruvyxa analyze`            | ตรวจสอบ routes, imports, server/client boundaries       |
| `npx ruvyxa doctor`             | ตรวจสอบ project setup, tools, dependencies, environment |
| `npx ruvyxa trace /blog/[slug]` | ดูรายละเอียด manifest ของหนึ่ง route                    |
| `npx ruvyxa bench`              | Benchmark route discovery, analysis + production build  |
| `npx ruvyxa test:parity`        | เปรียบเทียบ dev/prod routes + smoke-render page routes  |
| `npx ruvyxa parity`             | Alias `test:parity`                                     |
| `npx ruvyxa clean`              | ลบ `.ruvyxa/` build output                              |

## คำอธิบายแต่ละคำสั่ง

### `dev`

เริ่ม development server บน `localhost:3000` (ปรับด้วย `--host`, `--port`):

- HMR ผ่าน WebSocket — style/component อัปเดตโดยไม่ reload ทั้งหน้า
- Debug overlay ในเบราว์เซอร์
- Render cache: 1024 entries, TTL 5 นาที
- ตรวจจับ port conflict และ auto-scan 100 ports ถัดไป

### `build`

สร้าง production output ลง `.ruvyxa/` รองรับ `--target node`, `edge`, `static`:

1. ค้นพบและ validate routes
2. Compile client bundles + source maps
3. Pre-render SSG/ISR/PPR/CSR pages ผ่าน parallel worker pool
4. Optimize images → WebP
5. Emit build manifest + metadata

### `check`

ตรวจสอบความพร้อม production ในคำสั่งเดียว:

1. TypeScript type checking
2. Production build
3. Dev/prod route parity
4. Page smoke rendering

### `start` / `preview`

Serve production build จาก `.ruvyxa/` ด้วย runtime semantics เดียวกับ dev

### `routes`

พิมพ์ route table พร้อม rendering strategy ที่ถูก detect

### `analyze`

ตรวจสอบ 4 ด้าน:

1. Route structure — duplicate/ambiguous detection
2. Import graph — module resolution
3. Server/client boundary — `server-only`, `client-only`, private env
4. Route rendering strategy

### `doctor`

รายงาน 6 ด้าน:

1. Project root และ app directory
2. Node.js, pnpm, Rust versions
3. Dependencies installed?
4. Ruvyxa CLI binary
5. Route count และ discovery
6. Overall health signal

### `trace`

ดู manifest entry แบบละเอียดของหนึ่ง route — แสดง path, id, file, layout, strategy, params

### `bench`

วัดประสิทธิภาพของ:

- Route discovery
- Route analysis
- Production build (รวม prerender)
- แสดงผลเป็น milliseconds

ใช้ `--samples` เพื่อรันหลายครั้ง, `--json` เพื่อ output ในรูปแบบ JSON

### `test:parity`

เปรียบเทียบ route list ระหว่าง dev และ production build + smoke-render ทุก page route

### `clean`

ลบไดเรกทอรี `.ruvyxa/` ทั้งหมด

## Options

| Option      | ใช้กับ                    | คำอธิบาย                         |
| ----------- | ------------------------- | -------------------------------- |
| `--root`    | ทุกคำสั่ง                 | Project root path                |
| `--host`    | `dev`, `start`, `preview` | Bind host (default: `localhost`) |
| `--port`    | `dev`, `start`, `preview` | Bind port (default: `3000`)      |
| `--target`  | `build`                   | `node`, `edge`, `static`         |
| `--samples` | `bench`                   | จำนวนรอบ benchmark               |
| `--json`    | `bench`                   | Output เป็น JSON                 |

## Build Output

```text
.ruvyxa/
├── server/        # Server-side source code
├── client/        # Client bundles + manifest.json
├── assets/        # Static assets + WebP images
├── prerender/     # Pre-rendered HTML pages
├── manifest.json  # Route manifest
└── build.json     # Build metadata + security defaults + timing
```

`build.json` บันทึกเวลาของแต่ละ phase (discovery, validation, preparation, client bundling,
prerender) เป็นมิลลิวินาที และ client manifest มี budget แสดง 10 route ที่มี first-load ใหญ่สุด
(threshold 250 KiB สำหรับสังเกตการณ์ โดยไม่ทำให้ build ล้มเหลว)

## ลำดับการตรวจสอบ

1. `analyze` — routes, imports, boundaries
2. `typecheck` — TypeScript
3. `check` — readiness signal (ก่อน deploy)
4. `build` + `start` — ทดสอบ production ในเครื่อง

## Environment Variables ที่เกี่ยวข้อง

| Variable                   | หน้าที่                                                                  | ค่าเริ่มต้น             |
| -------------------------- | ------------------------------------------------------------------------ | ----------------------- |
| `RUVYXA_RENDER_CACHE_SIZE` | จำนวน render cache; `0` คือปิด cache และค่าที่เกิน 16,384 จะถูกจำกัด     | 1024 (dev), 512 (prod)  |
| `RUVYXA_BUILD_CACHE_DIR`   | เปลี่ยนตำแหน่ง build cache                                               | `.ruvyxa/cache/bundler` |
| `RUVYXA_WORKER_TIMEOUT_MS` | timeout ของ request/API stream ฝั่ง Rust/Node; รองรับ 1–2,147,483,647 ms | 30,000 ms               |
| `RUVYXA_MEMORY_LIMIT_MB`   | จุดเริ่มลด worker cache ตาม memory; ค่าไม่ถูกต้องหรือ `0` ใช้ค่าเริ่มต้น | 512 MiB                 |
| `RUVYXA_CACHE_MAX_ENTRIES` | จำนวน compiled bundle/module ที่เก็บสูงสุดต่อ worker                     | 256                     |
