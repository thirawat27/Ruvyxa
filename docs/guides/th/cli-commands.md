# CLI Commands

| คำสั่ง                         | หน้าที่                                    |
| ------------------------------ | ------------------------------------------ |
| `npm run dev`                  | Dev server + HMR + file watching           |
| `npm run build`                | Production build → `.ruvyxa/`              |
| `npm run start`                | Serve production build                     |
| `npm run typecheck`            | `tsc --noEmit`                             |
| `npm run check`                | Typecheck + build + parity + smoke render  |
| `npx ruvyxa preview`           | Preview production build (alias `start`)   |
| `npx ruvyxa routes`            | พิมพ์ตาราง routes + strategy               |
| `npx ruvyxa analyze`           | ตรวจสอบ routes, imports, boundaries        |
| `npx ruvyxa doctor`            | ตรวจสอบ project setup, tools, deps         |
| `npx ruvyxa trace /blog/:slug` | ดูรายละเอียดหนึ่ง route                    |
| `npx ruvyxa bench`             | Benchmark route discovery + build          |
| `npx ruvyxa test:parity`       | เปรียบเทียบ dev/prod routes + smoke render |
| `npx ruvyxa parity`            | Alias `test:parity`                        |
| `npx ruvyxa clean`             | ลบ `.ruvyxa/`                              |

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
