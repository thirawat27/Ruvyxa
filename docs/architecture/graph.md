# Route Discovery & Validation (`ruvyxa_graph`)

**File**: `crates/ruvyxa_graph/src/lib.rs` (1696 lines, single file)

Route discovery walks the `app/` filesystem directory, classifies files by naming convention,
detects rendering strategy from source code, validates server/client boundaries, and produces a
`RouteManifest`.

---

## Type Definitions

```rust
pub type RouteParams = BTreeMap<String, serde_json::Value>;
// JSON-shaped: catch-all segments → Value::Array
// Omitted optional catch-all → no entry

pub struct RouteManifest {
    pub app_dir: PathBuf,
    pub routes: Vec<RouteEntry>,
}

pub struct RouteEntry {
    pub id: String,                    // e.g. "app/blog/[slug]/page"
    pub path: String,                  // e.g. "/blog/[slug]"
    pub kind: RouteKind,               // Page | Api
    pub file: PathBuf,                 // absolute path to page/route file
    pub layout_chain: Vec<String>,     // route IDs of ancestor layouts
    pub server_modules: Vec<String>,   // sibling server.ts, action.ts
    pub client_modules: Vec<String>,   // sibling client.tsx
    pub runtime: RuntimeTarget,        // Node | Edge | Static (all Node currently)
    pub render: RenderMeta,
}

pub enum RouteKind { Page, Api }       // serde: kebab-case
pub enum RuntimeTarget { Node, Edge, Static }
pub enum RenderStrategy { Ssr, Ssg, Isr, Csr, Ppr } // default: Ssr

pub struct RenderMeta {
    pub strategy: RenderStrategy,
    pub revalidate: Option<u64>,       // ISR seconds
    pub has_static_params: bool,       // getStaticParams/staticParams export
    pub static_paths: Vec<String>,     // resolved static param combos
    pub has_dynamic_slots: bool,       // PPR Suspense boundaries
}

pub struct DiscoverOptions {
    pub app_dir: PathBuf,
    pub default_render_strategy: Option<RenderStrategy>,
    pub default_revalidate: Option<u64>,
}

pub struct ValidationReport {
    pub routes: usize,
    pub page_routes: usize,
    pub api_routes: usize,
    pub client_modules: usize,
    pub server_modules: usize,
    pub diagnostics: Vec<Diagnostic>,
    // is_ok() → diagnostics.is_empty()
}
```

---

## `discover_routes(options) → Result<RouteManifest>`

### Step 1: Guard

```
if !app_dir.exists() → RUV1001 "App directory not found"
```

### Step 2: Walk filesystem

```
WalkDir::new(&app_dir)
  .filter_entry: skip dirs starting with "_" or "@"
  .filter_map(Result::ok)
```

### Step 3: Match files

For each file entry, match `file_name`:

| File name                                     | RouteKind         |
| --------------------------------------------- | ----------------- |
| `page.tsx`, `page.jsx`, `page.md`, `page.mdx` | `Page`            |
| `route.ts`, `route.js`                        | `Api`             |
| Other                                         | `continue` (skip) |

**Note**: `action.ts`, `action.js`, `server.ts`, `server.js`, `client.tsx` are NOT matched here —
they are discovered as **sibling modules** of the matched route file.

### Step 4: Compute fields

**`path = route_path_from_dir(relative_dir)`**

1. Split `relative_dir` into components.
2. Filter to `Component::Normal` only:
   - **Drop** route groups `(name)` — parentheses, content ignored in URL.
   - **Drop** parallel slots `@name` — at-sign prefix, ignored.
3. For each remaining segment, call `route_segment(segment, is_last)`.
4. If no segments remain → `"/"`.
5. Join with `/`, prefix `/`.

**`route_segment(segment: &str, is_last: bool) → Result<String>`**

| Pattern                              | Classification     | Rules                                                                                        |
| ------------------------------------ | ------------------ | -------------------------------------------------------------------------------------------- |
| `[[...name]]`                        | Optional catch-all | Must be LAST segment. Strips `[[...` and `]]`. `validate_dynamic_name(name)`. Returns as-is. |
| `[...name]`                          | Required catch-all | Must be LAST segment. Strips `[...` and `]`. `validate_dynamic_name(name)`. Returns as-is.   |
| `[name]`                             | Dynamic param      | `validate_dynamic_name(name)`. Returns as-is.                                                |
| Contains `[`/`]` doesn't match above | Invalid            | → RUV1002                                                                                    |
| Plain text                           | Static             | Returns unchanged                                                                            |

**`validate_dynamic_name(name: &str) → Result<()>`**

- Must be non-empty
- Must not contain `[` or `]`
- Must not start with `.`

**`id = route_id(app_dir, file)`**

Strip `app_dir` prefix from `file`. Drop extension. Join components with `/`. Prepend `app/`. Filter
to `Component::Normal`.

**`layout_chain = layout_chain(app_dir, route_dir)`**

1. Start at `current = app_dir`.
2. If `current/layout.tsx` exists → push `route_id(app_dir, current/layout.tsx)`.
3. Walk `relative` components from `route_dir.strip_prefix(app_dir)`:
   - For each `Component::Normal`: `current.push(component)`.
   - If `current/layout.tsx` exists → push `route_id(...)`.
4. Return ordered list: root layout first, innermost last.

**`server_modules = sibling_modules(route_dir, &["server.ts", "server.js", "action.ts", "action.js"])`**

Check each filename at `route_dir/name`. Push path if exists.

**`client_modules = sibling_module(route_dir, "client.tsx")`**

Check if `route_dir/client.tsx` exists. Returns Vec of 0 or 1 elements.

### Step 5: Render detection (Page only)

Page routes call
`apply_rendering_defaults(detect_render_strategy(...), default_strategy, default_revalidate)`.

API routes → `RenderMeta::default()` (SSR).

### Step 6: Sort & dedup

Routess sorted by `path` then `id`.

### Step 7: Conflict detection

`detect_conflicts(routes)`:

1. Build `BTreeMap<match_shape, RouteEntry>`.
2. `route_match_shape(path)`:
   - `[[...name]]` → `*?`
   - `[...name]` → `*`
   - `[name]` → `:`
   - Literals → unchanged.
3. If collision found → RUV1003 with both route IDs in `affected_routes`.

Example: `/blog/[slug]` and `/blog/[id]` both map to `/blog/:` → conflict.

---

## `detect_render_strategy(file, layout_chain) → RenderMeta`

Ordered first-match. Stops at first match.

### 1. Client-Side Rendering (CSR)

```
"use client" directive in original source
  → RenderMeta { strategy: CSr, ..default() }
```

Reads **original** source (not stripped). Checks if first line after trimming starts with
`"use client"` or `'use client'`.

### 2. Partial Pre-Rendering (PPR)

```
export const ppr = true
  → RenderMeta { strategy: Ppr, has_dynamic_slots: true, ..default() }
```

### 3. Incremental Static Regeneration (ISR)

```
export const revalidate = <number>
  → RenderMeta { strategy: Isr, revalidate: Some(seconds), has_static_params: check_for_static_params(), ..default() }
```

Extracts via regex: `export const revalidate = (\d+)` (after stripping comments/strings).

### 4. Static Site Generation (SSG) — explicit

```
getStaticParams or staticParams export in source
  → RenderMeta { strategy: Ssg, has_static_params: true, ..default() }
```

Checks for `getStaticParams` or `staticParams` in export-names-only extracted code.

### 5. Static Site Generation (SSG) — automatic

```
No dynamic segments in path
  AND no dynamic data markers in reachable code (page + layout chain)
  → RenderMeta { strategy: Ssg, ..default() }
```

**Reachable code**: `collect_relative_graph(page_file + all layout files)` → concatenate → strip
strings/comments → check markers.

**Dynamic data markers** (any of these → NOT static):

- `fetch(`, `headers(`, `cookies(`, `searchParams`
- `Date.now(`, `Math.random(`
- `process.env.` (any runtime env read disqualifies static)

### 6. Server-Side Rendering (SSR) — default

```
None of the above
  → RenderMeta::default()  // strategy: Ssr, everything false/None
```

### `apply_rendering_defaults(render, default_strategy, default_revalidate)`

If `render.strategy` is not `Ssr` → return unchanged (explicit strategy wins).

If `default_strategy` is `Some` → apply it to meta. If ISR and `revalidate` not set → default to
60s.

---

## `validate_app(root, manifest) → Result<ValidationReport>`

Called after `discover_routes`. Scans source of every route for boundary violations.

### Page validation

For each Page route:

1. Read source. If `.md`/`.mdx` → skip default-export check (content compilation provides one).
2. Check `export default` exists → else RUV1004.
3. `collect_relative_graph(page_file + layout_chain)` → BFS relative imports.
4. Validate each module in graph via `validate_client_module()`.

### API validation

For each API route:

1. `collect_relative_graph(route_file)` → BFS relative imports.
2. Validate each module in graph via `validate_server_module()`.

### Explicit module validation

- Each `server_module` → `validate_server_module()`.
- Each `client_module` → `validate_client_module()`.

### `validate_client_module(source, file_path, root) → Vec<Diagnostic>`

| Check                  | Rule                                                                                                                          | Code    |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------------- | ------- |
| `"server-only"` import | Text scan for `import "server-only"` or `import 'server-only'` → error                                                        | RUV1007 |
| Private env access     | `process.env.<NAME>` where NAME does NOT start with `RUVYXA_PUBLIC_`. Also handles `process.env["<NAME>"]`. Skips `NODE_ENV`. | RUV1008 |
| `server/` dir import   | File path (canonicalized) starts with `<root>/server/`. Only project-root `server/`, not `app/server/`.                       | RUV1010 |

### `validate_server_module(source, file_path) → Vec<Diagnostic>`

| Check                  | Rule                                                                     | Code    |
| ---------------------- | ------------------------------------------------------------------------ | ------- |
| `"client-only"` import | Text scan for `import "client-only"` or `import 'client-only'` → warning | RUV1009 |

---

## `collect_relative_graph(entry: &Path) → BTreeSet<PathBuf>`

BFS from entry file. Collects the transitive closure of **relative** imports.

1. Queue: start with entry.
2. `visited: BTreeSet<PathBuf>`.
3. While queue not empty:
   - Pop front. Skip if visited.
   - Read source. Extract import specifiers via `import_specifiers(source)`.
   - For each specifier:
     - **Skip** if not starting with `.` (relative only, no bare/node_modules).
     - `resolve_relative_import(from, specifier)` → `Option<PathBuf>`.
     - If resolved, push to queue.
4. Return visited set.

### `import_specifiers(source: &str) → Vec<String>`

1. `code_for_import_specifiers(source)` — preserves strings that follow `from`, `import`, `import(`,
   `require(`. Strips other strings, template literals, block comments, line comments.
2. Scan lines for:
   - ` from "..."` or ` from '...'` → extract quoted specifier.
   - `import "..."` or `import '...'` at line start → extract quoted specifier.
   - `import(` → extract specifier from `import("...")`.
   - `require(` → extract specifier from `require("...")`.

### `resolve_relative_import(from: &Path, specifier: &str) → Option<PathBuf>`

Base = `from.parent() / specifier`. Probes candidates:

1. bare path (exact)
2. `<bare>.ts`, `<bare>.tsx`, `<bare>.js`, `<bare>.jsx`, `<bare>.md`, `<bare>.mdx`
3. `<bare>/index.ts`, `<bare>/index.tsx`, `<bare>/index.js`, `<bare>/index.jsx`, `<bare>/index.md`,
   `<bare>/index.mdx`

Returns first that `is_file()`. Attempts `canonicalize()`.

---

## Helper functions

### `private_env_reads(source) → BTreeSet<String>`

Scans for `process.env.NAME` and `process.env['NAME']`. Returns names not prefixed `RUVYXA_PUBLIC_`.
Excludes `NODE_ENV`.

Uses byte-level scanner. Skips strings, comments, template literals (but recurses into `${}`
expressions). Handles bracket notation with both single and double quotes.

### `code_without_strings_and_comments(source) → String`

Strips:

- Double-quoted strings (`"..."`) — handles escape sequences
- Single-quoted strings (`'...'`) — handles escape sequences
- Template literals (`` `...` ``) — handles `${}` nesting via depth counter
- Line comments `//` → EOL
- Block comments `/* ... */`

Used by `detect_render_strategy` step 5 (static candidate) and ISR regex.

### `code_for_export_scanning(source) → String`

Like `code_without_strings_and_comments` but preserves `export` keyword context. Returns lines
containing `export`s for pattern matching.

### Serialization

```rust
pub fn write_manifest(manifest: &RouteManifest, output_file: &Path) -> Result<()>
    // serde::to_writer_pretty → output_file

pub fn read_manifest(manifest_file: &Path) -> Result<RouteManifest>
    // serde::from_reader → RouteManifest
```

---

## Diagnostic codes (graph module)

| Code    | Condition                                               | Recommendation                                                                            |
| ------- | ------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| RUV1001 | App directory not found                                 | Create `app/` or set `appDir` in config                                                   |
| RUV1002 | Invalid dynamic segment (`[a b]`, `[]`, error brackets) | Use `[param]`, `[...rest]`, `[[...opt]]`                                                  |
| RUV1003 | Same match shape conflct                                | Rename one route segment; dynamic params at same level must use different static prefixes |
| RUV1004 | Page missing `export default`                           | Add default export to page component                                                      |
| RUV1007 | Server-only module imported into client graph           | Move server logic to `server/` dir or `action.ts`                                         |
| RUV1008 | Private environment variable used in client graph       | Rename to `RUVYXA_PUBLIC_*` or move to server-only code                                   |
| RUV1009 | Client-only module imported into server graph           | Remove client-only dependency from API/server code                                        |
| RUV1010 | Server directory module reached by client graph         | Do not import from `server/` in client-reachable code                                     |
