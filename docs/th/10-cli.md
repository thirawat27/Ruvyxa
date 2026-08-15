# CLI และ application script

> **เป้าหมายของ tutorial:** ใช้ CLI เป็น feedback loop ตั้งแต่ development ในเครื่องไปจนถึง release
> check **เริ่มจาก:** แอปที่มี route อย่างน้อยหนึ่งรายการ;
> [สร้าง app แรก](02-create-your-first-app.md) มีตัวอย่างให้ **Checkpoint:** ตรวจ route list,
> application check และ analyzer output ของแอปคุณ

[README](../../README.md) ที่ root คือภาพรวมโครงการที่ใช้อ้างอิงหลัก ภายใน Ruvyxa application ที่
สร้างแล้ว ให้ใช้ npm script ตามตารางด้านล่าง นี่คือ interface ที่ starter ทุกตัวเตรียมไว้และ
copy-paste ได้จริง โดยเฉพาะให้ใช้ `routes:json` และ `analyze:html` แทนการให้ผู้อ่านประกอบ flag หลัง
script ขึ้นเอง

| คำสั่งใน application                                                                                                                  | สิ่งที่รัน                              | วัตถุประสงค์                                                             |
| ------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------- | ------------------------------------------------------------------------ |
| `npm run dev`                                                                                                                         | `ruvyxa dev`                            | route watching และ hot reload                                            |
| `npm run build`                                                                                                                       | `ruvyxa build`                          | สร้าง production output                                                  |
| `npm run check`                                                                                                                       | `ruvyxa check`                          | ตรวจความพร้อมของ application                                             |
| `npm run start` / `npm run preview`                                                                                                   | `ruvyxa start` / `preview`              | serve หรือ preview local ของ build ที่มีอยู่                             |
| `npm run routes`                                                                                                                      | `ruvyxa routes`                         | route table แบบอ่านง่าย                                                  |
| `npm run routes:json`                                                                                                                 | route JSON command ที่ starter กำหนด    | route output สำหรับเครื่องอ่าน                                           |
| `npm run analyze`                                                                                                                     | `ruvyxa analyze`                        | validate route, import และ server/client boundary                        |
| `npm run analyze:html`                                                                                                                | HTML analysis command ที่ starter กำหนด | หน้าวิเคราะห์แบบ interactive ในเครื่อง                                   |
| `npm run adds -- form`                                                                                                                | `ruvyxa adds form`                      | scaffold application flow ที่รองรับ                                      |
| `npm run doctor`, `npm run clean`, `npm run trace -- /`, `npm run bench`, `npm run test:parity`, `npm run plugin -- create my-plugin` | `ruvyxa` command ที่ตรงกัน              | diagnose, ลบ output, ตรวจ route, benchmark, ตรวจ parity หรือสร้าง plugin |

## เลือก JavaScript runtime

project command ที่รัน JavaScript รับ `--runtime node|bun|deno`; ตัวอย่างเช่น
`npm run build -- --runtime deno` flag นี้ override `RUVYXA_RUNTIME` และ `runtime` ใน
`ruvyxa.config.ts` ดูลำดับ fallback และ Deno permission model ที่
[Configuration](07-configuration.md#runtime-selection)

## Scaffold starter feature ด้วย `adds`

`adds` รับได้หนึ่งตัวหรือหลายตัวจาก `form`, `data-table` และ `auth` เท่านั้น มันเขียนไฟล์ใต้
`appDir` ที่ตั้งค่าไว้ (โดยทั่วไปคือ `app/`) ไม่ใช่ข้าง `package.json` ให้เรียกผ่าน npm script
`adds` ที่ starter สร้างไว้ ชื่อพหูพจน์ช่วยแยก scaffold นี้ออกจากคำสั่งติดตั้ง package อย่าง
`npm add`

```bash
npm run adds -- form
npm run adds -- data-table
npm run adds -- auth

# เพิ่มตัวอย่างที่เป็นอิสระต่อกันในครั้งเดียว
npm run adds -- form data-table auth
```

| Scaffold     | ไฟล์ที่สร้าง                                                                          | สิ่งที่แสดงให้เห็น                                                                                                 | สิ่งที่ต้องเติมก่อน production                                                                                      |
| ------------ | ------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------- |
| `form`       | `app/form-example/page.tsx`, `app/form-example/action.ts`                             | native POST form, การ validate email/message ฝั่ง server, action handler และ `invalidate('contacts')`              | แทน action ตัวอย่างด้วย persistence, authorization, anti-abuse control และ success/error UX ของคุณ                  |
| `data-table` | `app/_components/ruvyxa/data-table.tsx`                                               | generic client component ที่ filter ข้อความ, click เพื่อ sort column, ระบุ row key และ custom cell renderer ได้    | ส่ง row/column จริงเข้าไป; เพิ่ม pagination, server filtering, authorization และ mutation เมื่อแอปต้องใช้           |
| `auth`       | `app/_server/auth.ts`, `app/__ruvyxa/auth/[...path]/route.ts`, `app/sign-in/page.tsx` | UI credentials sign-in, auth route สำหรับ GET/POST และ in-memory auth/rate-limit store สำหรับ development เท่านั้น | ติดตั้ง `@ruvyxa/auth`, register `auth.plugin`, ตั้ง environment ที่ต้องใช้ และแทน demo credential กับ memory store |

### Form: action ที่สร้างรับค่าอะไร

form จะ POST ไปยัง `submitContact` parser ฝั่ง server จะเปลี่ยน `email` เป็น lower case แล้ว
validate, ต้องมี `message` ยาว 10–2,000 ตัวอักษร แล้ว invalidate cache key `contacts` attribute ใน
browser เช่น `required`, `minLength` และ `maxLength` ช่วยให้ feedback ทันที แต่ action parser คือ
validation ที่ใช้ ตัดสินจริง เพราะ request สามารถข้าม HTML control ได้

```tsx
// app/form-example/action.ts — แทนที่ body ของ handler ตัวอย่าง
.handler(async ({ input, invalidate }) => {
  await contacts.insert(input) // persistence และ authorization ฝั่ง server ของคุณ
  invalidate('contacts')
  return { accepted: true, email: input.email }
})
```

### Data table: ใช้ generic component ที่สร้างมา

scaffold สร้างเฉพาะ component ไม่ได้สร้าง route หรือ fetch data ให้ import จาก page แล้วส่ง
row/column ที่มี type เข้าไป การ sort ทำใน client และเปรียบเทียบค่าที่แสดง ดังนั้น dataset
ขนาดใหญ่ควร query/filter ที่ server แทนการพึ่ง component นี้อย่างเดียว

```tsx
// app/users/page.tsx
import { DataTable, type DataColumn } from '../_components/ruvyxa/data-table'

type User = { id: string; name: string; role: 'admin' | 'member' }
const columns: readonly DataColumn<User>[] = [
  { key: 'name', label: 'Name' },
  { key: 'role', label: 'Role', render: (role) => <strong>{role}</strong> },
]

export default function UsersPage() {
  const rows: User[] = [{ id: 'u1', name: 'Ari', role: 'admin' }]
  return <DataTable rows={rows} columns={columns} rowKey="id" />
}
```

### Auth: ทำให้ scaffold ปลอดภัยก่อนใช้งาน

หลังเพิ่ม auth ให้ติดตั้ง package และ register runtime ที่สร้างใน configuration scaffold เอง
**ไม่ได้** ติดตั้ง package หรือแก้ `ruvyxa.config.ts` ให้

```bash
npm install @ruvyxa/auth
```

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { auth } from './app/_server/auth'

export default config({ plugins: [auth.plugin] })
```

```dotenv
# .env — ห้าม commit ค่าเหล่านี้
RUVYXA_AUTH_SECRET=replace-with-a-secret-of-at-least-32-characters
RUVYXA_AUTH_ORIGIN=https://app.example.com
RUVYXA_DEMO_USER=demo@example.com
RUVYXA_DEMO_PASSWORD=replace-this-demo-password
```

credentials provider ที่สร้างยอมรับเฉพาะ email/password ที่ตั้งข้างบน เป็นตัวอย่างที่รันได้ ไม่ใช่
user database หรือระบบ hash password ก่อน production build ให้แทน in-memory auth/rate-limit store
สำหรับ development ด้วย durable atomic implementation มิฉะนั้น auth package จะ fail closed ด้วย
`RUV3105`

### Conflict และ `--force`

ก่อนเขียน command จะตรวจ target file ทุกไฟล์ หากมีไฟล์ใดอยู่แล้ว จะหยุดด้วย `RUV2401` และไม่เขียน
scaffold ชุดนั้น ตรวจ path ที่รายงาน เก็บการแก้ไขที่ผู้ใช้เป็นเจ้าของ แล้วใช้ force
เฉพาะไฟล์ที่ตั้งใจ สร้างใหม่จริง:

```bash
npm run adds -- form --force
```

## Build เฉพาะ API ด้วย `build --server-only`

`ruvyxa build --server-only` สร้าง artifact สำหรับแอปที่มีเฉพาะ API โดยยังทำ configuration loading,
route discovery, validation, plugin build hook, server staging และ deploy adapter เหมือน build ปกติ
ทุกประการ และข้ามงานที่มีเฉพาะหน้า HTML เท่านั้นที่ใช้:

| สร้าง                                             | ข้าม                                               |
| ------------------------------------------------- | -------------------------------------------------- |
| `server/` (source ของ app, components และ server) | `client/` — route bundle และ `route-manifest.json` |
| `assets/` — ทุกไฟล์จาก `public/` แบบไม่ดัดแปลง    | การแปลง WebP และ responsive image variant          |
| `manifest.json`, `build.json`                     | `prerender/` — output ของ SSG, ISR และ PPR         |
| `deploy/` จาก adapter ที่เลือก                    | `robots.txt` และ `sitemap.xml`                     |
|                                                   | การเก็บ CSS ของหน้า                                |

Security header, body limit ของ API และ action, action rate limit, middleware และ diagnostics
ไม่เปลี่ยน เพราะทั้งหมดเป็นของฝั่ง server ซึ่ง mode นี้ยัง build อยู่

มีกฎสองข้อที่ตรวจก่อน stage output ใด ๆ ดังนั้น build ที่ถูกปฏิเสธจะไม่แตะ `dist/` เดิม:

- **`RUV1211`** — mode นี้รองรับเฉพาะ target `node` และ `bun` เพราะ `static` ไม่มี server และ edge
  adapter ยังไม่มี server-only output contract
- **`RUV1210`** — โปรเจกต์ที่มี page route ใด ๆ จะล้มเหลว พร้อมระบุ path แรกที่พบ หากปล่อยผ่าน
  artifact จะ deploy สำเร็จแล้วคืน 404 จึงตัดสินให้เป็น error ตั้งแต่ตอน build

```bash
ruvyxa build --server-only
ruvyxa build --server-only --target bun --adapter node
```

`build.json` บันทึก `"serverOnly": true` และตั้ง `"clientDir": null` การ build ซ้ำทับ output เดิมที่
เป็น full build ด้วย `--server-only` จะลบ `client/`, `prerender/` และไฟล์ discovery ที่ค้างอยู่
เพราะ atomic commit แทนที่ชุด build output ทั้งหมด

flag นี้เป็น opt-in `ruvyxa build` ที่ไม่ใส่ flag ยังทำงานเหมือนเดิมทุกประการ

## Baseline สำหรับ production build ที่ทำซ้ำได้

ใช้ baseline mode ก่อนและหลังแก้ compiler, cache, chunking, HMR หรือ adapter:

```bash
npm run bench -- --baseline --samples 3 --json
```

แต่ละ sample ทำงานในสำเนาโปรเจกต์ชั่วคราวใต้ `.ruvyxa/bench/` และมี cache ของตัวเอง โดยวัด 7
scenario ตามลำดับ dependency: cold build, warm build, first production route, CSS edit,
client-boundary edit, server-route edit และ leaf-route edit การแก้ทั้งหมด syntax-safe
และอยู่ภายในสำเนาเท่านั้น โหมดนี้ไม่แก้ ไม่ลบ และไม่ warm source หรือ build cache ของโปรเจกต์จริง
จากนั้นลบ temporary workspace เมื่อจบ แต่ละ sample

รายงานใช้ contract ชื่อคงที่ `ruvyxa.build-bench` และแยกเวอร์ชันไว้ใน `schemaVersion: 1` พร้อม cache
observation ของแต่ละ scenario และจะเขียนผลเมื่อ cold/warm output ผ่าน semantic artifact-equivalence
check แล้วเท่านั้น timestamp, cache counter และ timing field เป็น telemetry จึงถูก normalize
ระหว่างตรวจ ส่วน deployed code, asset และ manifest ยังอยู่ใน proof ครบ รายงานยังมี
`peakResidentBytes`, edit files, cache observations และจำนวน HMR `reloadFallbacks`; budget ที่อยู่ใน
fixture จะปฏิเสธผลที่ทำให้เข้าใจผิดหรือเกินขอบเขต ผู้ใช้ `bench --json` แบบเดิมยังได้ array shape
เดิม เพราะ baseline mode เป็น opt-in

## Application loop ที่แนะนำ

รันจาก root ของ application ที่สร้างแล้ว ไม่ใช่จาก framework monorepo นี้:

```bash
npm run dev
npm run routes
npm run check
npm run build
npm run test:parity
```

ใช้ `npm run routes:json` เมื่อต้องส่งข้อมูล route แบบ structured ให้เครื่องมืออื่น และเปิดรายงานจาก
`npm run analyze:html` เมื่อต้องตรวจ bundle, route, import หรือ boundary `clean` ลบ generated Ruvyxa
build output จึงอย่ารันกับ path ที่มี artifact ที่ดูแลเอง

## การรัน framework CLI จาก monorepo นี้

root ของ repository นี้ตั้งใจมี workspace script เช่น `pnpm build`, `pnpm check` และ `pnpm test` แต่
**ไม่มี** application script เช่น `npm run dev` หรือ `npm run routes` หากต้องการทดสอบ broad fixture
จาก repository root ให้เรียก CLI ผ่าน Cargo และระบุ fixture ให้ชัดเจน:

```bash
cargo run -p ruvyxa_cli -- dev --root examples/demo
cargo run -p ruvyxa_cli -- routes --root examples/demo
cargo run -p ruvyxa_cli -- check --root examples/demo
```

เมื่อดูแล framework เอง ให้รัน `cargo run -p ruvyxa_cli -- <command> --help` CLI ที่ตรวจแล้วมี
`dev`, `build`, `check`, `start`, `preview`, `routes`, `analyze`, `adds`, `doctor`, `clean`,
`trace`, `bench`, `test:parity` และ `plugin create`

## Repository script

root `package.json` กำหนด `build`, `check`, `test`, `prepare`, `check:cargo-lock`,
`check:oxc-lockstep`, `check:unused`, `check:template-mirrors`, `format`, `format:check`,
`format:staged`, `release:validate`, `release:bump`, `pack:smoke`, `test:full-flow` และ
`publish:dry-run` `check:unused` รัน [Knip](https://knip.dev) ตรวจ workspace ฝั่ง
JavaScript/TypeScript ทั้งหมด และ fail เมื่อพบไฟล์, export, type หรือ dependency ที่ไม่ได้ใช้;
`release:validate` ก็รันด้วย Ruvyxa โหลดโค้ดจำนวนมากตาม convention — route ใน `app/`, `plugins/`,
`ruvyxa.config.ts`, runtime file ที่ native CLI resolve ด้วย path — `knip.json`
จึงประกาศสิ่งเหล่านี้เป็น entry point แทนที่จะถือว่า ไม่ได้ใช้ TypeScript package ที่เผยแพร่กำหนด
`build`, `check`, `test`, `format` และ `prepack` อย่างสม่ำเสมอ; ดู package manifest
ที่เกี่ยวข้องสำหรับ test glob

**ก่อนหน้า:** [การเชื่อมต่อ](09-integrations-auth-data-and-realtime.md) · **ถัดไป:**
[Architecture](11-architecture.md)
