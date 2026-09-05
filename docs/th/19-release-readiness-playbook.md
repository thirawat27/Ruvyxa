# Release-readiness playbook

> **เป้าหมายของ tutorial:** ซ้อม release path หนึ่งแบบและตรวจ configuration ที่ขาดหายก่อน deploy
> **เริ่มจาก:** production plan จาก [Deploy, run และ operate](15-deploy-run-and-operate.md)
> **Checkpoint:** ทำ release gate ทุกข้อให้ครบสำหรับ delivery model ที่เลือก

ใช้หน้านี้เป็นเส้นทางสุดท้ายจาก application ที่ทำงานใน local ไปสู่ release candidate มันใช้เฉพาะ
command และ framework behavior ที่มีอยู่ใน repository นี้; การ upload, secret, health check และ
rollback ของ platform ยังเป็นของ host ที่คุณเลือก

## 1. เลือก delivery model ที่รองรับหนึ่งแบบ

| Delivery model               | Build command                                                                             | สิ่งที่ต้องรู้ก่อนเลือก                                                                                                                                                                    |
| ---------------------------- | ----------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Long-lived Node/Bun process  | `npm run build -- --adapter node` หรือ `npm run build -- --adapter bun`                   | ใช้สำหรับ SSR และ native realtime process เริ่มจาก built application ด้วย `npm run start`                                                                                                  |
| Self-hosted Deno process     | `npm run build -- --adapter deno`                                                         | รองรับ SSR, SSG, CSR, ISR, PPR และ API route copy `deploy/deno/` แล้วเริ่ม standalone server; native realtime ใช้ไม่ได้                                                                    |
| Static host                  | `npm run build -- --target static`                                                        | ทุก route ที่ต้องใช้ตอน runtime ต้อง prerender ได้ static output ไม่ตอบ arbitrary SSR request                                                                                              |
| First-party platform adapter | `npm run build -- --adapter vercel` (หรือ netlify/cloudflare/railway/render/firebase/aws) | ตรวจ output contract ของ adapter และตั้ง provider นอก Ruvyxa native realtime ถูกปฏิเสธสำหรับ serverless/static adapter ที่ระบุใน [การเชื่อมต่อ](09-integrations-auth-data-and-realtime.md) |

อย่าเลือก adapter เพราะชื่อเหมือน account ของคุณอย่างเดียว รัน `npm run doctor -- --adapter <name>`
ก่อน; นี่คือ CLI command ที่ทำมาเพื่อตรวจ adapter compatibility โดยไม่สร้าง artifact

## 2. ทำให้ configuration ปลอดภัยสำหรับ release

ใช้ production configuration pattern ใน [Configuration](07-configuration.md) ก่อน build:

- ตั้ง `site.url` หรือ `RUVYXA_SITE_URL` เป็น HTTPS origin จริง; อย่าเผยแพร่ preview URL เป็น
  canonical
- ให้ private value ทุกตัวที่ `requireEnv([...])` ต้องการใน build environment
- คง `build.map: false` ไว้ เว้นแต่นโยบาย release ของคุณอนุญาตให้เผยแพร่ source map ชัดเจน
- ตั้ง `trustedProxyIps` เฉพาะ IP/CIDR ของ proxy ที่อยู่หน้า app จริง
- เปลี่ยน auth memory store สำหรับ development เป็น `redisAuthStore`/`redisRateLimitStore` หรือ
  durable store อื่นก่อน deploy หลาย instance

## 3. รัน release gate

รัน command เหล่านี้จาก application root ตามลำดับ ทุกคำต้องสำเร็จก่อนคำถัดไป

```bash
npm run routes
npm run check
npm run build
npm run test:parity
```

`routes` ยืนยัน public surface ที่ค้นพบ `check` คือ application readiness gate `build` สร้าง target
artifact `test:parity` เปรียบเทียบ dev/prod route และ smoke-render page route; มันจับ
framework-route drift แต่ไม่แทน unit, integration, accessibility หรือ load test ของ application

## 4. Deploy และพิสูจน์ release

deploy artifact จาก adapter/target ที่เลือกผ่านกลไกปกติของ platform สำหรับ self-hosted long-lived
process ให้รัน project ที่ตั้งค่าเดียวกันด้วย:

```bash
npm run start
```

สำหรับ Deno adapter ให้ deploy directory `<outDir>/deploy/deno/` ที่ copy แล้ว และเริ่ม standalone
server จาก directory นั้นแทน:

```bash
deno run -A --no-prompt server/index.mjs
```

ดู artifact layout และ environment setting ที่
[คู่มือ platform adapter](20-platform-adapter-guide.md#node-bun-and-deno-copy-a-standalone-app)

จากนั้น probe แบบ explicit ที่ app ของคุณ implement: request `/`, dynamic page หนึ่งหน้า, protected
route หนึ่ง route, write action/API route ด้วย test data ที่ปลอดภัย และ health API route
ถ้าคุณสร้างมัน ตรวจ response status, body ที่คาด, security header และ structured log framework ไม่มี
`/health` endpoint แบบสากล จึงต้อง probe application route ที่คุณเป็นเจ้าของ

## 5. Operate และ rollback

บันทึก application version, adapter/target, เวลา release, canonical origin และลิงก์ build log alert
บน process availability และ health route ของคุณ ไม่ใช่แค่ build สำเร็จ หาก release ล้มเหลว ใช้
immutable-artifact rollback ของ host เพื่อคืน version ล่าสุดที่ดี แล้วเปรียบเทียบ `npm run routes`,
generated build output, configuration และ log ระหว่าง release อย่าล้าง shared cache หรือเปลี่ยน
database state เพียงเพราะ rollback: การทำเช่นนั้นมีผลต่อข้อมูลเฉพาะ app ซึ่ง Ruvyxa ไม่ได้จัดการ

## Sign-off checklist

- [ ] Target/adapter เข้ากันกับทุก route และ plugin ที่เปิด
- [ ] Secret เป็น private และถูกส่งให้ตอน build/runtime ตามที่ต้องใช้
- [ ] `routes`, `check`, `build` และ `test:parity` ผ่านจาก release commit
- [ ] deployed origin, API, auth path และ static asset ถูก probe แล้ว
- [ ] มี log/alert และผู้รับผิดชอบ platform rollback
- [ ] ทีมได้ทดสอบ failure path ของ application data store

**ก่อนหน้า:** [ขอบเขตเอกสารและแหล่งข้อมูล](18-documentation-scope-and-sources.md) · **ถัดไป:**
[คู่มือ platform adapter](20-platform-adapter-guide.md)
