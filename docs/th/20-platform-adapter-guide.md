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

| Adapter    | Target     | Runtime | Route ที่รองรับ              |
| ---------- | ---------- | ------- | ---------------------------- |
| AWS        | serverless | node    | SSR, SSG, CSR, ISR, PPR, API |
| Bun        | node       | bun     | SSR, SSG, CSR, ISR, PPR, API |
| Cloudflare | edge       | edge    | SSR, SSG, CSR, API           |
| Deno       | node       | deno    | SSR, SSG, CSR, ISR, PPR, API |
| Firebase   | serverless | node    | SSR, SSG, CSR, ISR, PPR, API |
| Netlify    | serverless | node    | SSR, SSG, CSR, ISR, PPR, API |
| Node       | node       | node    | SSR, SSG, CSR, ISR, PPR, API |
| Railway    | node       | node    | SSR, SSG, CSR, ISR, PPR, API |
| Render     | node       | node    | SSR, SSG, CSR, ISR, PPR, API |
| Static     | static     | static  | SSG, CSR                     |
| Vercel     | serverless | node    | SSR, SSG, CSR, ISR, PPR, API |

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

**เวอร์ชัน runtime ที่รองรับ** Node ใช้ค่า `engines.node` ใน package manifest ส่วน Bun ต้อง **1.1.26
ขึ้นไป** — รุ่นที่เพิ่ม `idleTimeout` ให้ `Bun.serve` ซึ่งเป็น API ใหม่สุดที่ server ที่ emit
ออกมาใช้ — และ Deno ต้อง **2.0 ขึ้นไป** ซึ่งเป็นรุ่นที่ Node built-in ที่มัน import (`node:process`,
`node:fs`, `node:path`) กลายเป็นทางที่รองรับจริง `ruvyxa doctor` รายงานเวอร์ชัน
ที่ติดตั้งของแต่ละตัวและเตือนเมื่อต่ำกว่าเกณฑ์ ทดสอบกับ Bun 1.4.0 และ Deno 2.9.5

server แต่ละตัวใช้ HTTP server ของ runtime ตัวเอง: `node:http` บน Node, `Bun.serve` บน Bun และ
`Deno.serve` บน Deno ทุกอย่างเหนือ transport เป็นโปรแกรมชุดเดียวกัน — URL ไหนหมายถึงไฟล์ไหน, serve
เป็นอะไร, cache ได้นานเท่าไร, range ขอ byte ช่วงไหน, routing หรือ publish directory ตอบก่อน และ
shutdown drain อย่างไร — ทั้งสามจึงตอบเหมือนกัน `RUVYXA_SHUTDOWN_GRACE` จำกัดเวลา drain ทั้งสามตัว
ส่วน `RUVYXA_KEEP_ALIVE_TIMEOUT` ยก keep-alive window ของ Node ให้สูงกว่า idle window ของ managed
proxy — ถ้าไม่ตั้ง Node จะปิด idle connection ที่ห้าวินาที แล้ว request ถัดไปของ proxy บน connection
นั้นจะพังเป็น 502 — และเมื่อตั้งค่าไว้จะจำกัด `idleTimeout` ของ Bun ด้วย (หน่วยวินาที สูงสุด 255)
นอกนั้นปล่อย Bun ไว้ที่ default ของมันเอง ซึ่งไม่ปิด idle connection เลยและไม่ตัด streamed response
ที่ยาว

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

| Platform   | Artifact ที่แน่นอน                                                                             | รายละเอียดเชิงปฏิบัติการ                                                                                                                                                                         |
| ---------- | ---------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Vercel     | `<outDir>/deploy/vercel/.vercel/output/`; project-root `.vercel/output/` โดยปริยาย             | มี static file, `__ruvyxa_handler.func`, function config และ route config serverless เป็น default; `vercel({ edge: true })` เลือก edge output                                                    |
| Netlify    | `<outDir>/deploy/netlify/` พร้อม project-root `.netlify/v1/` Frameworks API artifact โดยปริยาย | ISR/PPR ไม่อยู่ใน static publish เพื่อให้ request ไป function และ revalidate ได้ root `netlify.toml` สร้างเฉพาะ `projectConfig: true` และไม่เขียนทับ                                             |
| Cloudflare | `<outDir>/deploy/cloudflare/worker`, `assets/` และ `wrangler.jsonc`                            | Worker จัดการ dynamic traffic และ assets binding serve static file ใช้ `wrangler deploy -c .ruvyxa/deploy/cloudflare/wrangler.jsonc` `projectConfig: true` เขียน root config เฉพาะเมื่อไม่มีไฟล์ |

ให้ส่ง private environment value ผ่าน secret mechanism ของ host เหล่านี้ อย่า publish deploy
directory เป็น generic static site: provider config ที่สร้างคือสิ่งที่ route dynamic traffic ไป
runtime

## Railway และ Render

`railway()` เขียน root `railway.json` โดยปริยาย; `render()` เขียน root `render.yaml` โดยปริยาย
ทั้งคู่ไม่เขียนทับ user-maintained file configuration ที่สร้างใช้ `npm run build` และเริ่ม handler
เหล่านี้:

```text
node .ruvyxa/deploy/railway/server/index.mjs
node .ruvyxa/deploy/render/server/index.mjs
```

Railway config ที่สร้างใช้ Railpack และ `ON_FAILURE` ที่มี 10 retry Render Blueprint เลือก Node
`24.x` ล่าสุดด้วยช่วง `>=24.19.0 <25` handler ทั้งคู่ bind `0.0.0.0` และอ่าน `PORT` หากคุณดูแล
provider file เอง ให้ใช้ `projectConfig: false` และคง build/start relationship เดียวกัน

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
