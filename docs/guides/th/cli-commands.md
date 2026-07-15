# CLI Commands

| คำสั่ง                          | หน้าที่                                    |
| ------------------------------- | ------------------------------------------ |
| `npm run dev`                   | Dev server + HMR + file watching           |
| `npm run build`                 | Production build → `.ruvyxa/`              |
| `npm run start`                 | Serve production build                     |
| `npm run typecheck`             | `tsc --noEmit`                             |
| `npm run check`                 | Typecheck + build + parity + smoke render  |
| `npx ruvyxa preview`            | Preview production build (alias `start`)   |
| `npx ruvyxa routes`             | พิมพ์ตาราง routes + strategy               |
| `npx ruvyxa analyze`            | ตรวจสอบ routes, imports, boundaries        |
| `npx ruvyxa doctor`             | ตรวจสอบ project setup, tools, deps         |
| `npx ruvyxa trace /blog/[slug]` | ดูรายละเอียดหนึ่ง route                    |
| `npx ruvyxa bench`              | Benchmark route discovery + build          |
| `npx ruvyxa test:parity`        | เปรียบเทียบ dev/prod routes + smoke render |
| `npx ruvyxa parity`             | Alias `test:parity`                        |
| `npx ruvyxa clean`              | ลบ `.ruvyxa/`                              |

`build.json.timing` บันทึกเวลาของ route discovery, validation, preparation, client bundling,
prerender และเวลารวมเป็นมิลลิวินาที ใช้คู่กับ `ruvyxa bench` เพื่อหาขั้นตอนที่ควรตรวจสอบก่อน ปรับ
build settings

client manifest มี `budget` สำหรับแสดง 10 route ที่มี first-load ใหญ่ที่สุดเทียบกับ budget
สำหรับสังเกตการณ์ 250 KiB โดยไม่ทำให้ build ล้มเหลว และแต่ละ route มี `artifactCacheHit` เมื่อ reuse
client artifact ที่ fingerprint จาก dependency graph ได้

## Options

| Option     | ใช้กับ                    | คำอธิบาย                 |
| ---------- | ------------------------- | ------------------------ |
| `--root`   | ทุกคำสั่ง                 | Project root             |
| `--host`   | `dev`, `start`, `preview` | Bind host                |
| `--port`   | `dev`, `start`, `preview` | Bind port                |
| `--target` | `build`                   | `node`, `edge`, `static` |

## ลำดับการตรวจสอบ

1. `analyze` — routes, imports, boundaries
2. `typecheck` — TypeScript
3. `check` — readiness signal (ก่อน deploy)
4. `build` + `start` — ทดสอบ production ในเครื่อง

ดูฉบับเต็ม (อังกฤษ): [CLI Commands (EN)](../en/cli-commands.md)
