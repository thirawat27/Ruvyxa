# Wasm Middleware Plugins

สร้าง Rust starter สำหรับ Wasm middleware ABI ของ Ruvyxa:

```bash
npx ruvyxa plugin new request-logger
cd request-logger
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --release
cd ..
npx ruvyxa plugin debug request-logger
```

`plugin new` สร้าง `<name>/` ที่ตำแหน่งปัจจุบัน พร้อม `cdylib`, exported Wasm memory, allocator
`ruvyxa_alloc` และ hook `on_request` / `on_response` โดย starter เริ่มต้นจะคืนค่า `continue` จึง
compile และเปิดใช้งานเพื่อตรวจสอบได้อย่างปลอดภัย

`plugin debug <name>` จะหาไฟล์ build ของ `<name>` ให้อัตโนมัติ จึงใช้เพียง
`npx ruvyxa plugin debug request-logger` ได้เลย และยังระบุ path ที่ลงท้าย `.wasm` เองได้เมื่อจำเป็น
คำสั่งจะตรวจ module ด้วย Wasmtime engine เดียวกับ runtime แล้วรายงาน exports, `memory`, hook และ
allocator ต้องมี `memory` และอย่างน้อยหนึ่ง hook; หาก ABI ไม่เข้ากันจะจบด้วย `RUV2100`

เพิ่มไฟล์ที่ build แล้วใน `middleware.plugins` ของ `ruvyxa.config.ts` ตามตัวอย่างใน README ของ
starter. Plugin ยังคงอยู่ใน sandbox เดิม: ไม่มีสิทธิ์ filesystem หรือ network และ environment,
timeout, memory ต้องระบุอย่างชัดเจน.
