# ขอบเขตเอกสารและแหล่งข้อมูล

> **เป้าหมายของ tutorial:** รู้ว่าข้อความใดมี implementation และ test รองรับ
> จึงใช้ในแอปได้อย่างมั่นใจ **เริ่มจาก:** บทใดก็ได้ที่คุณต้องยืนยันความสามารถ **Checkpoint:** แยก
> framework contract ที่รองรับออกจาก implementation detail ที่เป็นของ provider ได้

หน้านี้จับคู่หัวข้อถาวรแต่ละส่วนกับ source tree ที่รับผิดชอบ และบทที่อธิบายพฤติกรรม
ที่ผู้ใช้เกี่ยวข้อง implementation path คือ source of truth; นี่ไม่ใช่การกล่าวอ้างว่า private
implementation ที่ไม่อยู่ในเอกสารเป็น public API

| Source area            | Implementation ที่รับผิดชอบ                                                                  | เอกสาร                                                                                                                                   |
| ---------------------- | -------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| CLI/config/build       | `crates/ruvyxa_cli/src/*` และ `packages/ruvyxa/runtime/{config-renderer,adapter-runner}.mjs` | [CLI](10-cli.md), [Configuration](07-configuration.md), [Deploy และ operate](15-deploy-run-and-operate.md)                               |
| Route graph            | `crates/ruvyxa_graph/src/lib.rs`                                                             | [โครงสร้างโปรเจกต์](03-project-structure.md), [Routing](04-routing-rendering.md)                                                         |
| Bundler/boundary       | `crates/ruvyxa_bundler/src/*`                                                                | [Architecture](11-architecture.md), [Security](13-security.md)                                                                           |
| Dev server             | `crates/ruvyxa_dev_server/src/*`                                                             | [Architecture](11-architecture.md), [Performance](14-observability-performance.md)                                                       |
| Middleware/diagnostic  | `crates/ruvyxa_middleware/src/*`, `crates/ruvyxa_diagnostics/src/lib.rs`                     | [Plugin](08-plugins-middleware.md), [Security](13-security.md)                                                                           |
| Terminal presentation  | `crates/ruvyxa_tui/src/*`                                                                    | [CLI](10-cli.md), [Architecture](11-architecture.md)                                                                                     |
| Core surface           | `packages/@ruvyxa/core/src/{index,types,server,config,plugin}.ts`                            | [ข้อมูล](05-data-actions-api.md), [Configuration](07-configuration.md), [API reference](17-public-api-reference.md)                      |
| React surface          | `packages/@ruvyxa/react/src/*`                                                               | [UI และ asset](06-ui-navigation-metadata-and-assets.md), [Routing](04-routing-rendering.md), [API reference](17-public-api-reference.md) |
| First-party plugin     | `packages/ruvyxa/src/plugins.ts`                                                             | [Plugin](08-plugins-middleware.md), [Observability](14-observability-performance.md)                                                     |
| Runtime/adapter        | `packages/ruvyxa/runtime/*`, `packages/@ruvyxa/adapter-*/src/index.ts`                       | [Architecture](11-architecture.md), [Deploy และ operate](15-deploy-run-and-operate.md)                                                   |
| Auth/database/realtime | `packages/@ruvyxa/{auth,database,realtime}/src/*`                                            | [การเชื่อมต่อ](09-integrations-auth-data-and-realtime.md), [Security](13-security.md)                                                    |
| Creation/testing       | `packages/create-ruvyxa/src/*`, template, `packages/@ruvyxa/testing/src/index.ts`, test      | [สร้าง app แรก](02-create-your-first-app.md), [Development และ testing](12-development-testing.md)                                       |
| Demo example           | `examples/demo/app/*`, `examples/demo/plugins/*`, `examples/demo/ruvyxa.config.ts`           | บท 03–09                                                                                                                                 |

## Inventory ของ command ที่ตรวจแล้ว

| Scope                 | Command/script                                                                                                                                                                                                  |
| --------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Ruvyxa CLI            | `dev`, `build`, `check`, `start`, `preview`, `routes`, `analyze`, `adds`, `doctor`, `clean`, `trace`, `bench`, `test:parity`, `plugin create`                                                                   |
| Generated application | `dev`, `build`, `start`, `preview`, `typecheck`, `check`, `routes`, `routes:json`, `analyze`, `analyze:html`, `adds`, `doctor`, `clean`, `trace`, `bench`, `test:parity`, `plugin`                              |
| Repository root       | `build`, `check`, `test`, `prepare`, `check:cargo-lock`, `check:oxc-lockstep`, `format`, `format:check`, `format:staged`, `release:validate`, `release:bump`, `pack:smoke`, `test:full-flow`, `publish:dry-run` |

## สิ่งที่ยืนยันว่าไม่มี/ยังไม่ใช่ framework feature

Ruvyxa ยังไม่ได้ implement **React Server Components** ไม่มี module graph แบบ `react-server` ไม่มี
client-reference manifest และไม่มี wire format ของ React Flight ส่วน `'use client'` เป็นการระบุ
module lane ที่ bundler บังคับใช้ ไม่ใช่ client reference ที่ React resolve เอง ส่วน `flight` export
และ `useFlight()` ของ Ruvyxa เป็น JSON payload ต่อ route สำหรับ soft navigation ซึ่งไม่มีอะไรร่วมกับ
RSC นอกจากชื่อ — ดู [Data, action และ API route](05-data-actions-api.md)

codebase ไม่มี public generic dependency-injection API, generic queue, scheduler, framework event
bus, database migration service, managed metrics backend, alert manager, backup/recovery
implementation, container/orchestrator manifest หรือ universal readiness endpoint
เอกสารนี้ระบุความไม่มีเหล่านี้แทนการสร้าง API หรือ deployment procedure ขึ้นเอง platform behavior
นอก first-party adapter contract ต้องตรวจจาก configuration ของ platform ที่เลือก

## วิธีตรวจเอกสาร

หลังแก้ไข ให้ตรวจว่า language tree ทั้งสองมี filename เหมือนกันและ Markdown link resolve แล้ว
จากนั้นรัน application/repository check ที่สัมพันธ์กับพฤติกรรมที่เปลี่ยน
งานเอกสารอย่างเดียวอย่างน้อยต้องตรวจ internal link และ paired-tree parity; code change ต้องใช้ check
ใน [Development และ testing](12-development-testing.md)

**ก่อนหน้า:** [Public API reference](17-public-api-reference.md) · **ถัดไป:**
[Release-readiness playbook](19-release-readiness-playbook.md)
