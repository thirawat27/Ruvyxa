# Troubleshooting และ compatibility เมื่ออัปเกรด

> **เป้าหมายของ tutorial:** วินิจฉัย command ที่ล้มเหลวจากหลักฐาน และอัปเกรดโดยไม่ข้าม compatibility
> check **เริ่มจาก:** command loop ใน [CLI](10-cli.md) **Checkpoint:** ทำให้อาการเกิดซ้ำ
> ใช้วิธีแก้ที่ตรงกัน แล้วรัน command ที่เคยล้มเหลวอีกครั้ง

รัน diagnostic ที่แคบที่สุดก่อน จาก application root:

```bash
npm run routes
npm run check
npm run analyze
npm run doctor
npm run trace -- /
npm run test:parity
```

## อาการและวิธีแก้ที่มีหลักฐานรองรับ

| อาการ                                         | เงื่อนไขที่เป็นไปได้                                                                          | ตรวจและแก้                                                                                         |
| --------------------------------------------- | --------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| route หาย                                     | ไฟล์ไม่ตาม special-file/dynamic-segment rule ที่ค้นหา                                         | รัน `routes`; เปรียบเทียบ directory/name กับ [โครงสร้างโปรเจกต์](03-project-structure.md)          |
| client build รายงาน private import/env access | boundary validation พบ server-only import หรือ environment value ที่ไม่ public ใน client path | ย้ายงานไป server-side; เปิดเผยเฉพาะ `RUVYXA_PUBLIC_*` ที่ปลอดภัยโดยเจตนา                           |
| static build ล้มเหลว                          | static adapter ไม่มี generated prerender page หรือ route ต้องใช้ runtime-only behavior        | ใช้ target ที่เข้ากัน หรือให้ static param/route strategy; ตรวจ build output                       |
| `RUV2102`                                     | plugin definition ไม่มี name/behavior หรือ hook shape ไม่ถูกต้อง                              | ให้ `definePlugin` มี `name` ไม่ว่างและ declaration/register callback ที่ถูกต้อง                   |
| `RUV3001`–`RUV3003`                           | database adapter input, mapping หรือ operation ทำไม่ได้                                       | ตรวจ `DatabaseAdapterError` message และ model/table mapping ของ adapter                            |
| `RUV3201`                                     | native realtime build สำหรับ target/adapter ที่ไม่รองรับ                                      | deploy long-lived Node/Bun output หรือเอา realtime ออก                                             |
| action/API ปฏิเสธ body                        | body เกิน action/API limit ที่ตั้ง หรือ input parser throw                                    | ดู `security.actionLimit`/`apiLimit`; validate และคืน application error ที่ปลอดภัย                 |
| cache ดูเก่า                                  | entry ยังใน TTL/SWR หรืออีก process มี memory cache ของตน                                     | ใช้ `invalidateCache`, ตรวจ strategy และใช้ shared infrastructure สำหรับข้อมูลหลาย instance        |
| `RUV1405`                                     | พบ PostCSS config แต่โหลด plugin หรือตัว `postcss` เองไม่ได้                                  | ติดตั้ง package ที่ config ระบุ หรือเอาออกจาก config                                               |
| `RUV1014`                                     | `--root` ชี้ไปยัง path ที่ไม่มีอยู่                                                           | แก้ path หรือรันคำสั่งจากในโปรเจกต์                                                                |
| `RUV1015`                                     | รัน `start`/`preview` ก่อน `build` จึงไม่มี compiled app ให้ serve                            | รัน `ruvyxa build` ก่อน หรือใช้ `ruvyxa dev` เพื่อ serve จาก source                                |
| `RUV1016`                                     | `RUVYXA_RUNTIME` ไม่ใช่ `node`, `bun` หรือ `deno` — ข้อความจะอ้างค่าที่ตั้งไว้กลับมาให้       | แก้หรือ unset ตัวแปร; `--runtime` override ได้ทีละคำสั่ง                                           |
| `RUV1406`                                     | PostCSS plugin throw หรือ stylesheet มี syntax error ที่ chain ปฏิเสธ                         | แก้ error ของ plugin/stylesheet ที่รายงาน build จะไม่ปล่อย CSS ที่ยังไม่ transform ออกไป           |
| `RUV1805`                                     | ไฟล์ `.json` ที่ import ไม่ใช่ JSON ที่ถูกต้อง                                                | ข้อความระบุไฟล์และตำแหน่งที่ parse ไม่ผ่าน ให้แก้เอกสารนั้น                                        |
| `RUV1806`                                     | import resolve ไปยังไฟล์ชนิดที่ Ruvyxa ไม่ compile (`.node`, `.wasm`, binary asset)           | เพิ่ม package นั้นใน `build.external` ให้ runtime โหลดไฟล์แทน bundler                              |
| `RUV1807`                                     | ตัวพิมพ์ของ import ไม่ตรงกับชื่อไฟล์บนดิสก์ ระบบไฟล์นี้ไม่แยกตัวพิมพ์ แต่ Linux แยก           | สะกด import ให้ตรงกับชื่อไฟล์ ข้อความจะระบุการสะกดทั้งสองแบบ                                       |
| `RUV1017`                                     | โฟลเดอร์ catch-all มี segment ลูกต่อท้าย ซึ่งไม่มี URL ใดไปถึงได้                             | ย้าย catch-all ไปไว้ท้าย route หรือลบ segment ที่อยู่ใต้มันออก                                     |
| `RUV1018`                                     | marker ของ intercepting route ไต่ระดับ URL มากกว่าที่ route ต้นทางมีอยู่                      | ใช้ marker ที่สั้นลง หรือใช้ `(...)` เพื่อระบุเป้าหมายจาก root ของ app                             |
| `RUV1019`                                     | page export ทั้ง `ppr` และ `serverComponents` ซึ่งเรนเดอร์ผ่านคนละ entry                      | เอา export ตัวใดตัวหนึ่งออกจาก page                                                                |
| `RUV1020`                                     | route ที่ใช้ `serverComponents` มี interception ซึ่ง registry ฝั่งไคลเอนต์ไม่ถูกประกาศไว้     | เอา `serverComponents` ออก หรือย้าย interception ไป route ที่ไม่ได้ใช้มัน                          |
| `RUV1021`                                     | อ่านไดเรกทอรีใต้ app directory ไม่ได้ จึงมองไม่เห็น route ที่อยู่ข้างใต้                      | ให้สิทธิ์อ่านแก่ build หรือย้ายไดเรกทอรีนั้นออกจาก `app/`; โฟลเดอร์ที่ขึ้นต้นด้วย `_` จะไม่ถูกเดิน |

**หน้าแสดงด้วย browser default ทั้งที่ class name ถูกต้อง** global stylesheet ถึง browser
โดยยังไม่ถูก transform ให้ตรวจว่ามี `@import "tailwindcss"` เหลืออยู่ใน CSS ที่เสิร์ฟหรือไม่
โปรเจกต์ที่ใช้ Tailwind v4 ต้องมี PostCSS config ที่ project root และติดตั้ง `postcss` ดู
[PostCSS และ Tailwind CSS](06-ui-navigation-metadata-and-assets.md#postcss-และ-tailwind-css)

**adapter build ล้มเหลวข้างใน package ที่คุณไม่ได้เขียน** SDK อ่านไฟล์ JSON, native addon หรือ asset
อื่นที่ไม่ใช่ JavaScript ซึ่ง deployment bundle ต้องพาไปด้วย JSON จะถูก compile เป็น data
ส่วนชนิดอื่นจะรายงาน `RUV1806` พร้อมชื่อไฟล์และ import ที่พาไปถึง serverless adapter จะ bundle
dependency ของ route เข้า function ปัญหาจึงเห็นตอน `ruvyxa build --adapter <name>` แต่ไม่เห็นตอน
`ruvyxa build` ธรรมดา

## คำถามที่พบบ่อย

**ควรใช้ not-found helper ตัวไหน?** `notFound()` จาก `@ruvyxa/react` throw tagged signal แล้ว route
boundary ที่ใกล้ที่สุด render `not-found.tsx` — ตัวนี้สำหรับ page และตรงกับ Next.js ส่วน
`status(404, message?)` จาก `ruvyxa/server` คืน `Response` ให้ handler ใต้ `app/api/` หรือ loader
ส่งกลับ ก่อนแยกชื่อกันทั้งคู่ชื่อ `notFound` เหมือนกัน page ที่ import ฝั่ง server จึงได้ `Response`
object มา render และ API route ที่ import ฝั่ง browser ก็ throw แทนที่จะตอบ

**ทำไม environment value หายจาก browser?** มีเพียง `RUVYXA_PUBLIC_*` ที่ตั้งใจให้ client ใช้ ย้าย
secret หรือ server-only computation ออกจาก client code แทนการเปลี่ยน prefix

**อัปเกรดอย่างปลอดภัยต้องทำอย่างไร?** ย้าย `ruvyxa` และ `@ruvyxa/*` ทุกตัวไปพร้อมกัน
เพราะปล่อยเป็นชุดเดียวกัน และคาดหวังเวอร์ชันที่ตรงกัน จากนั้นรัน `npm run check`, `npm run build`
และ `npm run test:parity` กับ app ของคุณตามลำดับนี้ `check` typecheck ก่อน config key หรือ export
ที่ไม่ได้อยู่ใน public surface แล้วจึงถูกรายงาน เป็นชื่อ แทนที่จะไปพังตอน runtime ส่วน `build` กับ
`test:parity` จับ behavior ที่ย้ายที่

คู่มือนี้อธิบาย framework ตามสภาพปัจจุบัน ไม่ใช่สภาพในอดีต ประวัติการเปลี่ยนแปลงราย release อยู่ที่
`CHANGELOG.md` ใน repository

**ก่อนหน้า:** [Deploy, run และ operate ใน production](15-deploy-run-and-operate.md) · **ถัดไป:**
[Public API reference](17-public-api-reference.md)
