# Wasm Middleware Plugins

Create a Rust starter that implements Ruvyxa's Wasm middleware ABI:

```bash
npx ruvyxa plugin new request-logger
cd request-logger
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --release
cd ..
npx ruvyxa plugin debug request-logger
```

`plugin new` creates `<name>/` in the current directory with a minimal `cdylib`, exported Wasm
memory, an optional `ruvyxa_alloc` allocator, and `on_request` / `on_response` hooks. The initial
implementation returns `continue`, so it is safe to compile, inspect, and enable before adding
policy logic.

`plugin debug <name>` finds `<name>/target/wasm32-unknown-unknown/release/<name>.wasm`
automatically, validates it with the same Wasmtime engine used at runtime, then reports its exports,
`memory`, request/response hooks, and allocator. An explicit `.wasm` path remains supported. It
requires `memory` plus at least one hook and exits with `RUV2100` when the ABI is incompatible.

Enable the compiled module in `ruvyxa.config.ts`:

```ts
export default config({
  middleware: {
    plugins: [
      {
        name: 'request-logger',
        phase: 'request',
        allow: { timeout: 5000, memory: 64 * 1024 * 1024 },
      },
    ],
  },
})
```

The module runs in the existing plugin sandbox. File-system and network permissions remain
unsupported; only explicitly allowed environment variables, timeout, and memory are available.
