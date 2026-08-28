# คู่มือ platform adapter

> **เป้าหมายของ tutorial:** จับคู่ build target ที่ทดสอบแล้วกับ artifact และ handoff ที่ platform
> ของคุณต้องการ **เริ่มจาก:** delivery model ที่เลือกใน
> [Release-readiness playbook](19-release-readiness-playbook.md) **Checkpoint:** ตรวจ generated
> artifact และทำ provider handoff checklist ให้ครบ

หน้านี้บันทึก deployment artifact ที่ first-party adapter สร้างจริง มันบอกว่า Ruvyxa เขียนอะไรและ
command ใดใช้เริ่มมัน แต่ไม่สร้างขั้นตอน dashboard, account, DNS, IAM หรือ billing ของ provider
ที่ไม่มีใน repository นี้ path ของ artifact ทุกอันอยู่ใต้ `outDir`; เมื่อใช้ default ให้ใช้
`.ruvyxa` แทน `<outDir>`

## เลือกและตรวจ adapter

ใช้ CLI เลือกแบบชั่วคราวระหว่างประเมิน host หรือ import typed adapter ใน `ruvyxa.config.ts`

```bash
npm run doctor -- --adapter railway
npm run build -- --adapter railway
```

```ts
import { config } from 'ruvyxa/config'
import { railway } from '@ruvyxa/adapter-railway'

export default config({ adapter: railway() })
```

CLI ตรวจ Vercel, Netlify, Cloudflare, Railway, Render และ AWS จาก build-environment marker variable
ได้เมื่อไม่ได้ตั้ง `RUVYXA_ADAPTER` ให้ระบุ `--adapter` ชัดเจนระหว่าง release test

## แผนที่ capability และ artifact

ตารางนี้สร้างจาก adapter release contract โดยตรง และ `pnpm release:validate` จะปฏิเสธเมื่อเอกสาร
drift

<!-- adapter-matrix:start -->

| Adapter    | Target     | Runtime | Route ที่รองรับ              | On-demand image |
| ---------- | ---------- | ------- | ---------------------------- | --------------- |
| AWS        | serverless | node    | SSR, SSG, CSR, ISR, PPR, API | ไม่ได้          |
| Bun        | node       | bun     | SSR, SSG, CSR, ISR, PPR, API | ไม่ได้          |
| Cloudflare | edge       | edge    | SSR, SSG, CSR, API           | ได้             |
| Deno       | node       | deno    | SSR, SSG, CSR, ISR, PPR, API | ไม่ได้          |
| Firebase   | serverless | node    | SSR, SSG, CSR, ISR, PPR, API | ไม่ได้          |
| Netlify    | serverless | node    | SSR, SSG, CSR, ISR, PPR, API | ไม่ได้          |
| Node       | node       | node    | SSR, SSG, CSR, ISR, PPR, API | ไม่ได้          |
| Railway    | node       | node    | SSR, SSG, CSR, ISR, PPR, API | ไม่ได้          |
| Render     | node       | node    | SSR, SSG, CSR, ISR, PPR, API | ไม่ได้          |
| Static     | static     | static  | SSG, CSR                     | ไม่ได้          |
| Vercel     | serverless | node    | SSR, SSG, CSR, ISR, PPR, API | ได้             |

<!-- adapter-matrix:end -->

native realtime ต้องใช้ long-lived Node/Bun output มันใช้ได้กับ Node, Bun, Railway และ Render
แต่ปฏิเสธ Deno, AWS, Cloudflare, Firebase, Netlify, static และ Vercel ดู
[การเชื่อมต่อ](09-integrations-auth-data-and-realtime.md)

## Node, Bun, and Deno: copy a standalone app

build ด้วย adapter แล้ว copy ทั้ง `deploy/node/`, `deploy/bun/` หรือ `deploy/deno/` ไป runtime image
หรือ host อย่า copy เฉพาะ server file: public asset เป็น sibling artifact standalone server
ทั้งสามไม่ต้องใช้ Ruvyxa CLI หรือ native binary ตอน runtime

```bash
npm run build -- --adapter node
PORT=3000 HOST=0.0.0.0 node .ruvyxa/deploy/node/server/index.mjs

npm run build -- --adapter bun
PORT=3000 HOST=0.0.0.0 bun .ruvyxa/deploy/bun/server/index.mjs

npm run build -- --adapter deno
PORT=3000 HOST=0.0.0.0 deno run -A --no-prompt .ruvyxa/deploy/deno/server/index.mjs
```

สองบรรทัด start ด้านบนใช้ syntax ตั้ง environment variable ของ POSIX หากใช้ PowerShell ให้ตั้งค่า
ก่อนเริ่ม artifact เดิมดังนี้:

```powershell
$env:PORT = '3000'
$env:HOST = '0.0.0.0'
node .ruvyxa/deploy/node/server/index.mjs

# หรือใช้ Bun output
bun .ruvyxa/deploy/bun/server/index.mjs

# หรือใช้ Deno output
deno run -A --no-prompt .ruvyxa/deploy/deno/server/index.mjs
```

generated server ทั้งสามใช้ `PORT=3000` และ `HOST=0.0.0.0` เป็น default adapter แต่ละตัวสร้าง
`start.mjs` ที่เริ่มผ่าน Ruvyxa CLI ที่ติดตั้งด้วย; เลือก standalone command เมื่อ runtime image
ไม่ควรมี CLI Deno standalone command ตั้ง permission ที่ server ต้องใช้โดยตั้งใจ จึงรันเฉพาะ
artifact ที่ build จาก project ที่คุณเชื่อถือ

**เวอร์ชัน runtime ที่รองรับ** Node ใช้ค่า `engines.node` ใน package manifest ส่วนเกณฑ์ขั้นต่ำของ
Bun และ Deno อยู่ที่ `MINIMUM_BUN_VERSION` และ `MINIMUM_DENO_VERSION` ใน
[`crates/ruvyxa_cli/src/environment.rs`](../../crates/ruvyxa_cli/src/environment.rs)
โดยแต่ละตัวบันทึกเหตุผลไว้ข้าง ๆ ค่า: ของ Bun คือรุ่นที่เพิ่ม `idleTimeout` ให้ `Bun.serve` ซึ่งเป็น
API ใหม่สุดที่ server ที่ emit ออกมาเรียกใช้ ส่วนของ Deno คือรุ่นที่ Node built-in ที่มัน import
(`node:process`, `node:fs`, `node:path`) กลายเป็นทางที่รองรับจริงแทนการเปิดด้วย flag

อย่าตรวจด้วยสายตา ให้ `ruvyxa doctor` อ่านเวอร์ชันที่ติดตั้งของแต่ละตัวและเตือนเมื่อต่ำกว่าเกณฑ์
ซึ่งเป็นเหตุผลทั้งหมดที่ค่าคงที่สองตัวนั้นมีอยู่

server แต่ละตัวใช้ HTTP server ของ runtime ตัวเอง: `node:http` บน Node, `Bun.serve` บน Bun และ
`Deno.serve` บน Deno ทุกอย่างเหนือ transport เป็นโปรแกรมชุดเดียวกัน — URL ไหนหมายถึงไฟล์ไหน, serve
เป็นอะไร, cache ได้นานเท่าไร, range ขอ byte ช่วงไหน, routing หรือ publish directory ตอบก่อน และ
shutdown drain อย่างไร — ทั้งสามจึงตอบเหมือนกัน `RUVYXA_SHUTDOWN_GRACE` จำกัดเวลา drain ทั้งสามตัว
ส่วน `RUVYXA_KEEP_ALIVE_TIMEOUT` ยก keep-alive window ของ Node ให้สูงกว่า idle window ของ managed
proxy — ถ้าไม่ตั้ง Node จะปิด idle connection ที่ห้าวินาที แล้ว request ถัดไปของ proxy บน connection
นั้นจะพังเป็น 502 — และเมื่อตั้งค่าไว้จะจำกัด `idleTimeout` ของ Bun ด้วย (หน่วยวินาที สูงสุด 255)
นอกนั้นปล่อย Bun ไว้ที่ default ของมันเอง ซึ่งไม่ปิด idle connection เลยและไม่ตัด streamed response
ที่ยาว

**Deadline** render ที่ไม่มีวันเสร็จจะถูกเลิกรอหลัง `RUVYXA_RENDER_TIMEOUT` มิลลิวินาที ค่าปริยาย
30000 — เท่ากับ window ที่ `ruvyxa start` ให้ worker round-trip หนึ่งครั้ง
โปรเจกต์เดียวกันจึงถูกจำกัดแบบเดียวกันทั้งตอนพัฒนาและตอน build ของตัวเอง คำตอบคือ `503` พร้อม
`Retry-After` เพราะ render ไม่ได้ล้มเหลว เซิร์ฟเวอร์แค่เลิกรอ และผู้เรียกที่ลองใหม่อาจได้รับบริการ
สิ่งที่ถูกจำกัดคือการรอ response เท่านั้น ไม่ใช่ body ที่ตามมา — streamed document และ
server-sent-event stream จึงทำงานได้นานเท่าที่ต้องการเมื่อ header ออกไปแล้ว ตั้ง
`RUVYXA_RENDER_TIMEOUT=0` เพื่อปิด สำหรับ deployment ที่ render นานกว่านั้นจริง ๆ และรู้ตัว
ถ้าไม่มีสิ่งนี้ connection หน่วยความจำ และสิ่งที่ render รออยู่ จะถูกถือไว้ตราบเท่าที่ process
ยังมีชีวิต — route แบบนั้นเพียงตัวเดียวภายใต้ ทราฟฟิกปกติจบลงด้วย out-of-memory kill โดยไม่มีอะไรใน
log บน Node connection ที่เริ่ม request แล้วส่งไม่จบจะถูกปิดตาม window ของ `RUVYXA_HEADERS_TIMEOUT`
เพราะ `headersTimeout` ของ Node เองไม่ทำงานกับรูปแบบนี้ ส่วน Bun ปิดเอง และ Deno ไม่มี API ให้ทำ —
deployment ของ Deno ที่เปิดสู่อินเทอร์เน็ตโดยตรงจึงควรมี reverse proxy ด้านหน้า

**Capacity** render ที่เกินกำลังเครื่องจะถูกปฏิเสธแทนที่จะเริ่มทำ `RUVYXA_MAX_CONCURRENCY`
คือจำนวนที่รันพร้อมกัน — ค่าปริยายคือส่วนแบ่ง core ของ container จำกัดไว้ระหว่างสองถึงแปด เพราะ
render ใช้ CPU เป็นหลัก การรับเกินกว่านั้นมีแต่ทำให้ตัวที่กำลังทำอยู่ช้าลง — ส่วน `RUVYXA_MAX_QUEUE`
คือจำนวนที่รอได้ ค่าปริยายสี่ตัวต่อหนึ่งช่อง เกินจากนั้นผู้เรียกจะได้ `503` พร้อม `Retry-After`
แทนที่จะถูกพักไว้บนหน่วยความจำที่ process นี้ต้องแบก ถ้าไม่จำกัด
ทราฟฟิกที่พุ่งเกินกำลังเครื่องจะกลายเป็น heap ที่ถือ render ทุกตัวไว้พร้อมกัน
และความล้มเหลวไม่ใช่เซิร์ฟเวอร์ช้า แต่เป็น out-of-memory kill ที่ลาก request
ที่เกือบเสร็จแล้วลงไปด้วย `ruvyxa start` ไม่เคยมีปัญหานี้ เพราะ render worker ของมันถูกจำกัดด้วย
controller ตัวเดียวกัน และตอนนี้ standalone server ใช้ตัวเดียวกันแล้ว

ช่องจะถูกถือไว้จนกว่า **response** จะเกิดขึ้น ไม่ใช่จนกว่า body จะจบ streamed document และ
server-sent-event stream จึงกินช่องแค่ระดับมิลลิวินาที ไม่ใช่ตลอดอายุของมัน — stream
พร้อมกันร้อยตัวบนค่าปริยายถูกเสิร์ฟครบ ส่วนไฟล์ static ไม่เข้าสู่ admission เลย
หน้าเว็บที่กำลังพังจึงไม่ลาก stylesheet ของตัวเองลงไปด้วย ตั้ง `RUVYXA_MAX_CONCURRENCY=0` เพื่อปิด
admission สำหรับ deployment ที่มีอย่างอื่นด้านหน้าทำหน้าที่นี้อยู่แล้ว

**Compression** ทั้งสามตัวบีบอัด response ที่เป็น text — document, JSON, JavaScript, CSS, SVG — ด้วย
gzip เมื่อ client รับได้ และประกาศ `Vary: Accept-Encoding` บนทุก response ที่บีบอัดได้ เพื่อให้
shared cache แยก key ได้ถูก ประเภทที่บีบอัดมาแล้ว (รูป วิดีโอ ฟอนต์) ถูกปล่อยไว้เหมือนเดิม
เช่นเดียวกับ byte range ซึ่ง offset ของมันอ้างถึง byte ที่ยังไม่ถูก encode ตั้ง
`RUVYXA_COMPRESSION=0` เมื่อมี proxy หรือ CDN ด้านหน้าบีบอัดอยู่แล้ว การบีบอัดซ้ำเปลืองแต่ CPU
ของเครื่องที่เล็กกว่า การเจรจา encoding ถือว่า `q=0` คือการปฏิเสธ ตรงกับ `ruvyxa start` ส่วน brotli
ตั้งใจไม่รองรับที่นี่ เพราะไม่มี format ของ `CompressionStream` การรองรับจะทำให้ runtime
ตัวหนึ่งบีบอัดได้ดีกว่าอีกสองตัว สำหรับ build เดียวกัน

## Static hosting: publish เฉพาะ static output

```bash
npm run build -- --target static
```

publish folder ปริยายคือ `<outDir>/static/` factory `static` รับเฉพาะ relative directory ที่ไม่ว่าง
และไม่ทับ protected build folder เนื่องจาก `static` เป็น reserved word เมื่อเรียก function โดยตรง
ให้ import ด้วย alias เช่น `staticOutput` แล้วเรียก `staticOutput({ outputDir })` ให้ publish folder
นั้น `_headers` ถูกสร้างสำหรับ host ที่รู้จักไฟล์นี้; host ที่ไม่สนใจไม่ได้รับผล หาก build ปฏิเสธ
route ให้คง route เป็น static/CSR หรือเลือก server-capable adapter—อย่า publish static build ที่ทำ
SSR/API behavior ของคุณไม่ได้

## Vercel, Netlify และ Cloudflare

| Platform   | Artifact ที่แน่นอน                                                                 | รายละเอียดเชิงปฏิบัติการ                                                                                                                                                                                                                                                                                                                |
| ---------- | ---------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Vercel     | `<outDir>/deploy/vercel/.vercel/output/`; project-root `.vercel/output/` โดยปริยาย | มี static file, `__ruvyxa_handler.func`, function config และ route config serverless เป็น default; `vercel({ edge: true })` เลือก edge output                                                                                                                                                                                           |
| Netlify    | `<outDir>/deploy/netlify/` พร้อม project-root `netlify.toml` โดยปริยาย             | ISR/PPR ไม่อยู่ใน static publish เพื่อให้ request ไป function และ revalidate ได้ root `netlify.toml` เขียนครั้งเดียวและไม่เขียนทับไฟล์เดิม — Netlify ไม่มีช่องทางอื่นที่จะรู้ publish directory `projectConfig: false` เปลี่ยนไปใช้ `.netlify/v1/` Frameworks API แทน สองอย่างเป็นทางเลือก เพราะส่งทั้งคู่จะได้ function สองตัวที่ `/*` |
| Cloudflare | `<outDir>/deploy/cloudflare/worker`, `assets/` และ `wrangler.jsonc`                | Worker จัดการ dynamic traffic และ assets binding serve static file ใช้ `wrangler deploy -c .ruvyxa/deploy/cloudflare/wrangler.jsonc` `projectConfig: true` เขียน root config เฉพาะเมื่อไม่มีไฟล์                                                                                                                                        |

ให้ส่ง private environment value ผ่าน secret mechanism ของ host เหล่านี้ อย่า publish deploy
directory เป็น generic static site: provider config ที่สร้างคือสิ่งที่ route dynamic traffic ไป
runtime

### ISR และ PPR บน Cloudflare

Worker ไม่มี filesystem เอกสารที่ revalidate แล้วจึงต้องเก็บใน Workers KV ระบุชื่อ binding แล้ว
adapter จะได้ทั้งสอง strategy ถ้าไม่ระบุ มันจะประกาศว่าไม่รองรับทั้งคู่
โปรเจกต์ที่ใช้อย่างใดอย่างหนึ่ง จึงถูกปฏิเสธตั้งแต่ build ด้วย `RUV2202` แทนที่จะถูก deploy ไปเป็น
Worker ที่เรนเดอร์ใหม่ทุก request แล้วรายงานว่า cache hit

```ts
import { cloudflare } from '@ruvyxa/adapter-cloudflare'
import { config } from 'ruvyxa/config'

export default config({ adapter: cloudflare({ isr: { kvBinding: 'RUVYXA_ISR' } }) })
```

สร้าง namespace ครั้งเดียวด้วย `wrangler kv namespace create RUVYXA_ISR` แล้วเอา id ที่ได้ไปใส่ใน
`wrangler.jsonc` ที่ระบบสร้างให้ id ผูกกับบัญชี ไฟล์ที่ generate จึงใส่ placeholder ไว้แทนของจริง

### On-demand revalidation บน Vercel

route แบบ ISR บน Vercel ถูกเสิร์ฟด้วย Prerender Function ซึ่ง cache อยู่ "หน้า" function
ไม่ใช่ข้างใน `revalidatePath()` จึงต้องยิงถึง CDN และ Vercel รับ credential เดียวสำหรับเรื่องนี้คือ
prerender bypass token ที่ adapter เขียนลงใน `<route>.prerender-config.json` แต่ละไฟล์ และลงใน
function ที่ส่ง token กลับไป

ตั้ง `RUVYXA_PREVIEW_SECRET` ใน build environment adapter จะ hash ค่านั้นเป็น token
ตัวค่าจริงจึงไม่เคย ไปโผล่ใน output ที่ deploy

ถ้าไม่ตั้ง token จะ derive จาก build id ซึ่งทำให้ `ruvyxa build` ยัง reproducible แต่ก็แปลว่า token
เป็น ฟังก์ชันของสิ่งที่ build เปิดเผยอยู่แล้ว ทั้งชื่อ asset, path ของเอกสาร และตาราง route
ให้ถือว่าค่า default นี้ สะดวกสำหรับ preview deployment ไม่ใช่ความลับ ใครถือ token ก็สั่งข้าม CDN
cache ได้ทุก request

### Edge Function ต่อ route บน Vercel

route ที่ประกาศ `export const runtime = 'edge'` จะรันบน Node เหมือน route อื่นถ้าไม่ได้สั่งให้
adapter ไปวาง เพราะ Node ตอบ edge route ได้ถูกต้อง — การประกาศเป็นการ*จำกัด*สิ่งที่ route ทำได้
ไม่ใช่การเปลี่ยนว่าใครตอบได้ เปิด `splitEdgeRoutes` เมื่อ latency ของ route นั้นต้องการ point of
presence จริงๆ ไม่ใช่แค่ region:

```ts
export default config({ adapter: vercel({ splitEdgeRoutes: true }) })
```

ตัว build จะสร้างฟังก์ชันที่สอง `__ruvyxa_edge.func` ซึ่ง `.vc-config.json` ระบุ edge runtime และ
registry ที่คอมไพล์ไว้มีเฉพาะ route เหล่านั้น ส่วน Build Output config จะส่ง path ของมัน
ไปที่ฟังก์ชันนี้ก่อน catch-all ที่เหลือยังไปที่ฟังก์ชัน Node เหมือนเดิม

route จะผ่านเกณฑ์เมื่อไม่ต้องพึ่งสิ่งที่เฟรมเวิร์กเก็บไว้ที่เดียว server component และ server action
ตอบผ่าน `/__ruvyxa/rsc`, `/__ruvyxa/flight` และ `/__ruvyxa/action` ซึ่งเป็น path
เดี่ยวที่ฟังก์ชันเดียวเป็นเจ้าของ ส่วน ISR และ PPR ต้องการที่เก็บที่เขียนได้ซึ่งฟังก์ชัน edge ไม่มี
route ที่ประกาศ `edge` แล้วต้องใช้อย่างใดอย่างหนึ่งจะถูกปฏิเสธพร้อมระบุชื่อด้วย `RUV2203`
แทนที่จะถูกทิ้งไว้บน Node เงียบๆ — เพราะการถูกเมินเงียบๆ คือวิธีที่การตัดสินใจเรื่อง latency
เลิกเป็นจริงโดยไม่มีใครรู้

ฟังก์ชัน Node ยังคงถือทุก route รวมถึงตัวที่เป็น edge ด้วย เพราะมันคือ catch-all และ pattern ของ
path ไม่ใช่ตัว router — `/edge/` ที่มี trailing slash ไม่แมตช์ `^/edge$`
การถือไว้ทั้งสองที่แปลว่าคำขอแบบนั้น จะได้หน้าที่ถูกต้องจาก runtime ที่ผิด ดีกว่าได้ 404 จาก runtime
ที่ถูก

## Railway และ Render

`railway()` เขียน root `railway.json` โดยปริยาย; `render()` เขียน root `render.yaml` โดยปริยาย
ทั้งคู่ไม่เขียนทับ user-maintained file configuration ที่สร้างใช้ `npm run build` และเริ่ม handler
เหล่านี้:

```text
node .ruvyxa/deploy/railway/server/index.mjs
node .ruvyxa/deploy/render/server/index.mjs
```

Railway config ที่สร้างใช้ Railpack และ `ON_FAILURE` ที่มี 10 retry Render Blueprint ตรึง Node
ไว้ที่เมเจอร์ที่ framework ระบุไว้ใน `engines.node` ทำให้ Render เลือก patch ใหม่สุดในเมเจอร์นั้น
handler ทั้งคู่ bind `0.0.0.0` และอ่าน `PORT` หากคุณดูแล provider file เอง ให้ใช้
`projectConfig: false` และคง build/start relationship เดียวกัน

## Firebase และ AWS Amplify Hosting

`firebase()` สร้าง `<outDir>/deploy/firebase/public`, Functions bundle, function `package.json` และ
`firebase.json`; root `firebase.json` ถูกสร้างโดยปริยายแต่ไม่เขียนทับ README ที่ adapter สร้างให้
handoff command ที่ยืนยันแล้ว:

```bash
npm run build -- --adapter firebase
firebase deploy --only hosting,functions
```

`aws()` เขียน Amplify `.amplify-hosting/` static-plus-compute bundle โดยปริยายที่ project root
และใต้ `<outDir>/deploy/aws/` deploy manifest route static asset ไป static hosting และ dynamic
traffic ไป compute resource `default` compute runtime ปริยายคือ `nodejs24.x`; ค่า `nodejs20.x` และ
`nodejs22.x` เก่ายังคงใช้ได้เมื่อกำหนด compatibility override โดยตรง ตั้ง `projectOutput: false`
เฉพาะเมื่อ build system อื่นเก็บ deploy artifact

## เขียน adapter สำหรับ platform ที่ Ruvyxa ไม่ได้ ship มาให้

adapter คือ factory ที่คืน object ซึ่งมี `name`, `target`, `supports` (ไม่บังคับ) และ `build(ctx)`
ที่คืน `AdapterOutput` กลไกนี้ไม่ได้สงวนไว้ให้เฉพาะ adapter ใน repository นี้:
`ruvyxa build --adapter <package>` จะ resolve `@ruvyxa/adapter-<name>`, `ruvyxa-adapter-<name>`
และชื่อ package ตรง ๆ ตามลำดับ ดังนั้นแค่ publish `ruvyxa-adapter-flyio` ก็ทำให้ `--adapter flyio`
ใช้งานได้

```ts
import type { Adapter, BuildContext } from '@ruvyxa/core'
import {
  clientBuildOutput,
  runtimeBuildPolicy,
  standaloneServerSource,
  validateBuildContext,
} from '@ruvyxa/core'

export interface FlyAdapterOptions {
  appName?: string
}

export default function flyio(options: FlyAdapterOptions = {}): Adapter {
  const appName = options.appName ?? 'ruvyxa-app'
  return {
    name: 'flyio',
    target: 'node',
    supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
    build(ctx: BuildContext) {
      validateBuildContext(ctx, 'flyioAdapter')
      return {
        name: 'flyio',
        target: 'node',
        platform: 'flyio',
        runtime: 'node',
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        artifacts: [
          {
            kind: 'function',
            path: 'deploy/flyio/server',
            handlerSource: standaloneServerSource({ runtimePolicy: runtimeBuildPolicy(ctx) }),
          },
          { kind: 'static-site', path: 'deploy/flyio/public', optional: true },
          {
            kind: 'file',
            path: 'fly.toml',
            scope: 'project',
            skipIfExists: true,
            contents: `app = "${appName}"\n`,
          },
        ],
      }
    },
  }
}
```

กติกาสี่ข้อที่ runner บังคับ และอีกหนึ่งข้อที่ควรทำ:

- **รับ options object ก้อนเดียวและใส่ค่า default ให้ทุก field** factory จะถูกเรียกด้วย
  `config.adapterOptions` เมื่อ adapter ถูกเลือกด้วยชื่อ และด้วย `{}` เมื่อไม่ได้เลือกแบบนั้น option
  ที่ factory ปฏิเสธจะทำให้ build ล้มเหลวพร้อมข้อความของคุณเอง ซึ่งเป็นที่ที่ validation ควรอยู่
- **`platform` เป็น string อะไรก็ได้** ชื่อข้างบน autocomplete ได้ ส่วน platform ที่ package
  นี้ไม่เคยรู้จักก็เขียนลงไปตรง ๆ
- **`scope: 'project'` เขียน "ข้าง ๆ" project ไม่ใช่ "ทับ"** path ที่ resolve ออกนอก project root
  หรือทับ source directory, manifest, lockfile, `tsconfig.json`, `ruvyxa.config.*` หรือ
  `appDir`/`outDir` ที่ตั้งไว้ จะถูกปฏิเสธด้วย `RUV2200` ที่เหลือใน project root เป็นของคุณ —
  ซึ่งเป็นที่ที่ platform มองหาไฟล์ config ของมัน ใช้คู่กับ `skipIfExists: true` เพื่อให้ไฟล์ที่
  user เขียนเองชนะเสมอ
- **ประกาศ `supports` ตามจริง** runner จะ validate ทุก route เทียบกับรายการนี้ และปฏิเสธ route
  ที่ไม่รองรับพร้อมระบุชื่อ (`RUV2202`) แทนที่จะปล่อยให้ deployment 404 ตอน runtime
- **ใช้ `standaloneServerSource` ซ้ำ เว้นแต่ platform ต้องการ signature ของตัวเอง** มันคือ server
  ที่ CI รันจริงบน Node, Bun และ Deno ส่วน wrapper ที่เขียนเองคือโค้ดที่มีแต่ test ของคุณครอบคลุม

ตรวจผลลัพธ์โดยไม่ต้อง deploy:

```bash
ruvyxa build --adapter ruvyxa-adapter-flyio
```

## Provider handoff checklist

- deploy adapter artifact ไม่ใช่ raw application source file
- ให้ private environment value ตอน build/runtime ตามที่ app ต้องใช้
- ตั้ง provider health probe ไป application route ที่คุณ implement
- review `netlify.toml`, `wrangler.jsonc`, `railway.json`, `render.yaml` และ `firebase.json`
  ที่มีอยู่; adapter ตั้งใจ preserve
- รัน [Release-readiness playbook](19-release-readiness-playbook.md) แล้ว probe static, dynamic,
  API, authenticated และ error route แยกกันหลัง deploy

**ก่อนหน้า:** [Release-readiness playbook](19-release-readiness-playbook.md) · **ถัดไป:**
[Practical recipes](21-practical-recipes.md)
