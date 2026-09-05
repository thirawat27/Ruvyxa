# Development และ testing

> **เป้าหมายของ tutorial:** ตั้ง contributor loop และเลือก test
> ที่เล็กที่สุดซึ่งพิสูจน์การเปลี่ยนแปลงได้ **เริ่มจาก:** boundary map ใน
> [Architecture](11-architecture.md) **Checkpoint:** รัน check ที่แคบที่สุดซึ่งเกี่ยวข้องก่อนเลือก
> repository gate ที่กว้างขึ้น

## การตั้งค่าสำหรับ framework contributor

นี่คือ Rust workspace พร้อม pnpm workspace ติดตั้ง Node version และ pnpm ที่ประกาศไว้ จากนั้นใช้
Rust toolchain ที่เข้ากับ locked workspace

```bash
pnpm install
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --locked -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
pnpm lint
pnpm format:check
pnpm check:unused
pnpm release:validate
pnpm pack:smoke
```

`pnpm release:validate` คือ gate ตัวครอบ: มันรัน `lint` แล้วตามด้วย repository check แต่ละตัว —
package metadata, release publish plan, `oxc` lockstep, Markdown link, path ของ repository
ที่ถูกอ้างใน source comment, doc-comment attachment, silent default, cross-language constant,
template mirror, adapter sync และ Knip ระหว่างทำงานให้รันตัวที่แคบกว่าโดยตรง;
เงื่อนไขว่าควรรันตัวไหนเมื่อใด อยู่ใน repository guide [`AGENTS.md`](../../AGENTS.md)

สำหรับ broad fixture ให้ใช้คำสั่งที่ repository guide กำหนดไว้ตรงตัว:

```bash
cargo run -p ruvyxa_cli -- check --root examples/demo
cargo run -p ruvyxa_cli -- test:parity --root examples/demo
```

## Test layer

Rust test อยู่กับ crate ที่เกี่ยวข้องและครอบคลุม CLI/graph/bundler/server behavior package test
รันผ่าน Node built-in test runner; package manifest ชี้ไปที่ `tests/packages/**` หรือ package-local
test `@ruvyxa/react` มี test สำหรับ client router `@ruvyxa/testing` ให้ unit test สร้าง
loader/action/cache double และตรวจ call กับ invalidation ได้

```ts
import test from 'node:test'
import assert from 'node:assert/strict'
import { mockAction } from '@ruvyxa/testing'

test('records invalidation', async () => {
  const save = mockAction(({ input, invalidate }) => {
    invalidate('todos')
    return input
  })
  await save({ title: 'Write docs' })
  assert.deepEqual(save.invalidations, ['todos'])
})
```

repository ปัจจุบันมี CI workflow ที่ `.github/workflows/ci.yml` และ `.github/workflows/release.yml`
อย่ากล่าวอ้าง command ของ job รายตัวโดยไม่อ่าน workflow ใน revision ที่กำลังแก้ เพราะ workflow
เปลี่ยนแยกจาก package script ได้

## Worker-pool change matrix

| สิ่งที่เปลี่ยน                            | เจ้าของหลัก                                      | การพิสูจน์แบบเจาะจง                                                                            |
| ----------------------------------------- | ------------------------------------------------ | ---------------------------------------------------------------------------------------------- |
| Admission, fairness, queue bound, close   | `runtime/worker-admission.mjs`                   | `node --test tests/packages/ruvyxa/worker-admission.test.mjs`                                  |
| Node dispatch, rendering, cache, protocol | `runtime/worker-pool.mjs`                        | `node --test tests/packages/ruvyxa/worker-pool.test.mjs`                                       |
| Process selection, replacement, transport | `crates/ruvyxa_dev_server/src/worker_pool.rs`    | `cargo test -p ruvyxa_dev_server worker_pool --locked`                                         |
| Runtime file หรือ import                  | package manifest, pack smoke, CLI artifact cache | `node --test tests/packages/ruvyxa/worker-runtime-contract.test.mjs` แล้วรัน `pnpm pack:smoke` |

เริ่มจากแถวที่เป็นเจ้าของ behavior ที่เปลี่ยน จากนั้นรัน `pnpm --filter ruvyxa test` และ Rust crate
test ที่เกี่ยวข้อง local import ใหม่จาก `worker-pool.mjs` ต้องถูก publish โดย
`packages/ruvyxa/package.json` และ fingerprint โดย `crates/ruvyxa_cli/src/artifact_cache.rs`
เพื่อให้ CLI ที่ติดตั้งแล้ว load ได้ และ prerender ไม่ reuse output เก่า หากเป็นไปได้ให้เปลี่ยน
protocol แบบ additive และ update ทั้ง Rust serde type กับ Node test เมื่อ field เปลี่ยน

## Definition of done

สำหรับ public framework change ให้ update Rust/TypeScript contract, test, template เมื่อเกี่ยวข้อง
และเอกสารทั้งสองภาษาใน `docs/` รัน test ที่แคบที่สุดระหว่าง iteration แล้วรัน check
ที่กว้างขึ้นด้านบนก่อนส่งมอบ ห้าม commit generated `.ruvyxa/`, `dist/`, `target/`, `node_modules/`
หรือ package smoke directory

**ก่อนหน้า:** [Architecture](11-architecture.md) · **ถัดไป:** [Security](13-security.md)
