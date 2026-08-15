# Observability และ performance

> **เป้าหมายของ tutorial:** สังเกต request จริงก่อนปรับ rendering หรือ cache behavior ของมัน
> **เริ่มจาก:** security baseline ใน [Security](13-security.md) **Checkpoint:** เก็บ trace หรือ
> metric signal แล้วเปลี่ยนเฉพาะ bottleneck ที่ signal ระบุ

## Observability

ใช้ first-party plugin `observability()` เพื่อเพิ่ม request identifier, W3C `traceparent`,
`Server-Timing` และ structured record ต่อ response request-id header ปริยายคือ `x-request-id`; trace
context, server timing และ logging เปิดโดยปริยาย scope ได้กับ exact/trailing-star route และส่ง
custom logger ได้

```ts
import { config } from 'ruvyxa/config'
import { observability } from 'ruvyxa/plugins'

export default config({
  plugins: [
    observability({ routes: ['/api/*'], logger: (entry) => console.info(JSON.stringify(entry)) }),
  ],
})
```

record มี `requestId`, `traceparent`, `method`, `pathname`, `status` และ `durationMs` logger
ที่ล้มเหลวถูก isolate จึงไม่ทำให้ response ที่ปกติกลายเป็น HTTP failure ให้มองว่านี่คือฐานสำหรับ
telemetry sink ของคุณ ไม่ใช่ metrics/tracing backend ที่สมบูรณ์ ใน generated application ให้ใช้
`npm run analyze:html` สำหรับ local build/route analysis page และ `npm run trace -- /` เพื่อตรวจ
route manifest entry

สำหรับ correlated trace ระหว่างพัฒนา ให้เปิด `debug.traces` แล้วรัน `ruvyxa dev` response เดิมจาก
`/__ruvyxa/trace?path=/docs` ยังใช้ตรวจ route หนึ่งรายการ ส่วน `/__ruvyxa/trace?kind=edits`
ใช้ดูประวัติ edit ภายใน process ที่มีขอบเขต หรือเติม `path=` เพื่อกรอง ตามไฟล์ที่เปลี่ยนแบบ
project-relative แต่ละ edit ใช้ `traceId` เดียวตลอด graph classification, cache invalidation, worker
invalidation/replacement, HMR broadcast และการรับใน browser การตอบรับจาก browser รับเฉพาะ
same-origin, จำกัดขนาด และเปิดเฉพาะเมื่อ watch mode กับ `debug.traces` ทำงานพร้อมกัน trace response
ใช้ `no-store` และเป็น diagnostic ไม่ใช่ telemetry backend แบบถาวร

## `instrumentation.ts`

ไฟล์ชื่อ `instrumentation.ts` (หรือ `.js`/`.mjs`) ที่รากโปรเจกต์จะถูกรันหนึ่งครั้งต่อหนึ่ง process
ของเซิร์ฟเวอร์ ก่อนให้บริการคำขอแรก นี่คือที่ติดตั้ง observability SDK ระดับ process — plugin
`observability()` ข้างบนจัดรูป response แต่ละตัว ส่วนไฟล์นี้รันการตั้งค่าที่ SDK
ต้องมีก่อนจะจัดรูปอะไรได้

```ts
// instrumentation.ts
export async function register(): Promise<void> {
  const { NodeSDK } = await import('@opentelemetry/sdk-node')
  new NodeSDK({ serviceName: 'my-app' }).start()
}
```

เรียกเฉพาะ `register` ที่ export ออกมาเท่านั้น มันทำงาน:

- ใน render worker ภายใต้ `ruvyxa dev` และ `ruvyxa start` หนึ่งครั้งต่อหนึ่ง worker process
- ในทุก function instance หลัง deploy โดยเป็น top-level `await` ใน route registry ที่ถูก generate
  ก่อนที่ route module ใดจะถูกใช้

ตำแหน่งนี้คือสาระสำคัญ telemetry ต้องถูกติดตั้งใน process ที่ render จริง ๆ การรันมันใน CLI ที่สร้าง
worker จะเป็นการ instrument ผิด process

ความล้มเหลวจะถูก log แล้วกลืนไว้ และไฟล์ที่ไม่ export `register` จะถูกแจ้งทาง stderr แทนที่จะถูก
เพิกเฉย ทั้งสองอย่างเป็นเจตนา: telemetry มีไว้สังเกตเว็บที่ทำงานอยู่ exporter
ที่ตั้งค่าผิดจึงต้องไม่ใช่ เหตุผลที่เว็บหยุดให้บริการ — แต่ hook
ที่เงียบและไม่ทำอะไรเลยก็หน้าตาเหมือน hook ที่ทำงานอยู่ทุกประการ

`register` ถูก `await` ดังนั้นจะไม่มีคำขอไหนถูกให้บริการก่อนมันทำงานเสร็จ ควรทำให้เร็ว เพราะคำขอแรก
เป็นคนจ่ายค่านี้

ภายใน `register()` ให้เขียนด้วย `console.error` แทน `console.log` ใน Node worker standard output
คือช่องสัญญาณ NDJSON ที่ worker ใช้ตอบคำขอ บรรทัดที่เขียนลงไปจากที่อื่น จะทำให้ response
ที่คำขอกำลังรออยู่เสียหาย

## Performance control

- เลือก route strategy อย่างตั้งใจ: SSR สำหรับ HTML สดทุก request; SSG สำหรับ build output คงที่;
  ISR สำหรับ freshness ตามเวลา; CSR สำหรับ UI ใน browser; PPR สำหรับ static shell กับ dynamic
  section ที่ stream
- ใช้ `cache(key).ttl(...).swr(...)` สำหรับ data reuse ใน process ที่มีขอบเขต และ invalidate หลัง
  write มันไม่มี cross-process coherence
- เลือก `build.split: 'route'` เมื่ออยากได้ route-level code splitting; วัดก่อนบังคับ `single` หรือ
  `manual`
- build control มี `minify`, `treeShake`, `map`, `workers`, `warm` และ `prerenderCache` image
  control มี quality, lossless mode, variant, worker count และ on-demand transform
- `minify` ตัด comment ทั่วไปและ JSDoc ทิ้ง แต่เก็บ legal comment ไว้ — อะไรก็ตามที่ขึ้นต้นด้วย
  `/*!` หรือ `//!` หรือมี `@license` หรือ `@preserve` — แล้วรวบไปไว้ท้าย bundle แต่ละก้อน licence
  notice ของ dependency จึงเดินทางไปพร้อมโค้ดที่ต้องใช้มัน
- worker runtime มี request coalescing และ operational environment control เริ่มจาก default แล้วใช้
  load test และข้อมูล memory/latency ก่อนเปลี่ยน pool size, concurrency, queue capacity, timeout
  หรือ memory limit `RUVYXA_WORKER_MAX_CONCURRENCY` จำกัดงาน active ต่อ process และ
  `RUVYXA_WORKER_MAX_QUEUE` จำกัดงานที่รอ เมื่อ overload ระบบจะคืน `RUV1705` แทนการเก็บ request
  เพิ่มโดยไม่มีขอบเขต

สำหรับ framework diagnostic ค่า snapshot จาก `ping` ภายใน worker รายงานจำนวน request ที่ active และ
queued, limit ที่ตั้งไว้, rejection สะสม, retained module URL และขนาด cache หาก queue
ไม่กลับเป็นศูนย์หรือ rejection เพิ่มต่อเนื่อง แสดงว่าเกิด saturation ก่อนเพิ่ม limit ให้วัด CPU,
heap, tail latency และ rejection rate ร่วมกัน เพราะ queue ที่ใหญ่ขึ้นรับ burst ได้นานขึ้น แต่จะเก็บ
request body มากขึ้นและเพิ่มเวลารอด้วย

snapshot เดียวกันมี `cacheBudget` และ `compilerCache` โดย `cacheBudget` รายงาน hard, soft และ
hysteresis-target bytes, heap pressure ปัจจุบัน, pressure event และ eviction counter แยกตาม owner
`RUVYXA_MEMORY_LIMIT_MB` กำหนด hard limit ของ worker (ค่าเริ่มต้น 512 MiB) soft pressure จะ evict
LRU bundle entry ที่ไม่ถูก pin และล้าง derived module/compiler memory ส่วน hard pressure จะหยุด
speculative warmup เพิ่มด้วย key ที่มี active build lock จะไม่เป็น candidate สำหรับ eviction

## ข้อควรระวังเรื่อง cache และ concurrency

core cache ป้องกัน growth ไม่จำกัดที่ 1024 entry และคืน stale value ได้ขณะที่มี background refresh
หนึ่งงาน stale producer error จะเก็บ stale data เมื่อมี; cold failure ยัง throw plugin middleware
worker ไม่ share module state realtime reconnect เป็น client-side และ serverless adapter ไม่ host
native WebSocket realtime ข้อจำกัดเหล่านี้สำคัญเมื่อ scale เกิน process เดียว

**ก่อนหน้า:** [Security](13-security.md) · **ถัดไป:**
[Deploy, run และ operate ใน production](15-deploy-run-and-operate.md)
