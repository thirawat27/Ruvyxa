/**
 * The compile-time environment, as `import.meta.env`.
 *
 * Both compilers substitute this expression during compilation — the Rust one
 * for the browser bundle, `runtime/compiler.mjs` for every server render — and
 * the object they inline holds `RUVYXA_PUBLIC_*` names and nothing else, so a
 * private value cannot reach a browser through it.
 *
 * The runtime has behaved this way for a while; the type did not exist, so the
 * configuration chapter in each language's docs documented a form that failed
 * `tsc` with `Property 'env' does not exist on type 'ImportMeta'` — including
 * the paragraph telling the reader not to add private names to
 * `ImportMetaEnv`, which named an interface nothing declared.
 *
 * A project narrows this to the names it actually publishes, which is what
 * turns a typo into a compile error rather than `undefined` in the markup:
 *
 * ```ts
 * // env.d.ts in the project
 * interface ImportMetaEnv {
 *   readonly RUVYXA_PUBLIC_APP_NAME: string
 * }
 * ```
 *
 * Declaring a name without the `RUVYXA_PUBLIC_` prefix here does not publish
 * it. The prefix is enforced by the compilers, not by this file.
 */
interface ImportMetaEnv {
  readonly [name: `RUVYXA_PUBLIC_${string}`]: string | undefined
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
