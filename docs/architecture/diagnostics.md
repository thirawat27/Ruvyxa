# Diagnostic Codes Reference

Every framework error has a structured `RUV####` code with title, explanation, source location, and
suggested fix. Defined in `ruvyxa_diagnostics` and raised by every crate.

---

## Diagnostic struct

```rust
pub struct Diagnostic {
    pub code: &'static str,            // "RUV1007"
    pub title: &'static str,           // human-readable one-liner
    pub explanation: String,           // what went wrong, why
    pub span: Option<SourceSpan>,      // file:line:col
    pub import_chain: Vec<String>,     // trace for boundary violations
    pub suggested_fix: String,         // actionable fix text
    pub affected_routes: Vec<String>,  // route IDs impacted
}

pub struct SourceSpan {
    pub file: PathBuf,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

pub enum RuvyxaError {
    Diagnostic(Box<Diagnostic>),
    Io { message: String, source: Option<Arc<std::io::Error>> },
    Message(String),
}

pub type Result<T> = std::result::Result<T, RuvyxaError>;
```

---

## Graph Diagnostics (RUV1xxx)

Raised by `ruvyxa_graph`.

| Code        | Title                   | Condition                                                                                  | Fix                                                                  |
| ----------- | ----------------------- | ------------------------------------------------------------------------------------------ | -------------------------------------------------------------------- |
| **RUV1001** | App directory not found | `app/` directory missing at project root                                                   | Create `app/` dir or set `appDir` in config to an existing directory |
| **RUV1002** | Invalid route segment   | Dynamic segment syntax error: `[a b]`, `[]`, `[.name]`, brackets inside plain text segment | Use `[param]`, `[...rest]`, or `[[...rest]]`                         |
| **RUV1003** | Conflicting route paths | Two routes map to same match shape (e.g. `/blog/[slug]` and `/blog/[id]` both → `/blog/:`) | Differentiate paths with unique static prefix segments               |
| **RUV1004** | Missing default export  | Page component file has no `export default`                                                | Add `export default function Page() { ... }` to the page file        |

## Boundary Diagnostics (RUV1xxx)

Raised by `ruvyxa_graph` and `ruvyxa_bundler`.

| Code        | Title                            | Condition                                                                       | Fix                                                                                            |
| ----------- | -------------------------------- | ------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| **RUV1007** | Server-only in client graph      | `import "server-only"` detected in client-reachable module                      | Move server logic to `server/` dir, `action.ts`, or remove server-only import from client code |
| **RUV1008** | Private env var in client        | `process.env.VARIABLE` (not `RUVYXA_PUBLIC_*`) in client bundle                 | Rename to `RUVYXA_PUBLIC_VARIABLE` or move env read to server-only code                        |
| **RUV1009** | Client-only in server graph      | `import "client-only"` in API/server module                                     | Remove client-only dependency from server-side code                                            |
| **RUV1010** | Server directory in client graph | File under project-root `server/` directory imported by client-reachable module | Move importable logic out of `server/` dir, or keep imports server-side                        |

## Server Runtime Diagnostics (RUV11xx–RUV16xx)

Raised by `ruvyxa_dev_server`.

### SSR Errors

| Code        | Title                  | Condition                                                | Fix                                                               |
| ----------- | ---------------------- | -------------------------------------------------------- | ----------------------------------------------------------------- |
| **RUV1100** | React SSR failed       | `renderToString()` threw in a JavaScript worker          | Check component for errors, missing imports, invalid JSX          |
| **RUV1102** | SSR renderer not found | `runtime/ssr-renderer.mjs` missing in JavaScript workers | Reinstall `ruvyxa` package or verify runtime scripts are included |

### API Errors

| Code        | Title                      | Condition                                               | Fix                                                                   |
| ----------- | -------------------------- | ------------------------------------------------------- | --------------------------------------------------------------------- |
| **RUV1200** | API route execution failed | API handler threw uncaught error in a JavaScript worker | Check API route for errors, verify request shape matches expectations |
| **RUV1201** | No available server port   | Could not bind to port after 100 fallback attempts      | Free a port in range or change configured port                        |
| **RUV1202** | API renderer not found     | `runtime/api-renderer.mjs` missing                      | Reinstall `ruvyxa`                                                    |

### Client Bundle Errors

| Code        | Title                      | Condition                                          | Fix                                                        |
| ----------- | -------------------------- | -------------------------------------------------- | ---------------------------------------------------------- |
| **RUV1300** | Client bundling failed     | Bundle compilation error during dev client request | Check the page file and its imports for compilation errors |
| **RUV1303** | Client route not found     | Requested route path not in manifest               | Check route file exists and follows naming convention      |
| **RUV1304** | Client bundle for non-page | Client bundle requested for API route              | API routes don't have client bundles; only page routes     |

### Style Errors

| Code        | Title                        | Condition                                                     | Fix                                               |
| ----------- | ---------------------------- | ------------------------------------------------------------- | ------------------------------------------------- |
| **RUV1402** | Sass compilation failed      | `grass` Sass compiler failed (syntax error, import not found) | Fix Sass syntax or ensure imported files exist    |
| **RUV1403** | Stylesheet import unresolved | CSS `@import` or Sass `@use` has unresolvable path            | Use valid relative path or install the dependency |

### SSG / ISR / Action / PPR Errors

| Code        | Title                 | Condition                            | Fix                                                     |
| ----------- | --------------------- | ------------------------------------ | ------------------------------------------------------- |
| **RUV1500** | SSG render failed     | Static generation render threw error | Check the page component for runtime errors             |
| **RUV1501** | Action file not found | Server action handler file missing   | Create the `action.ts` file or fix the action name      |
| **RUV1550** | PPR render failed     | Partial pre-rendering failed         | Check for errors in static or dynamic parts of the page |

### Config Validation Errors

| Code        | Title                  | Condition               | Fix                                                 |
| ----------- | ---------------------- | ----------------------- | --------------------------------------------------- |
| **RUV1601** | Config value too small | Limit value ≤ 0         | Set positive value for body limit, rate limit, etc. |
| **RUV1602** | Config value too large | Limit value exceeds MAX | Reduce value to within allowed bounds               |

## Middleware & Plugin Diagnostics (RUV2xxx)

Raised by `ruvyxa_middleware`.

| Code        | Title                       | Condition                                                                                           | Fix                                                                                 |
| ----------- | --------------------------- | --------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| **RUV2000** | Middleware config error     | Invalid middleware configuration (bad header name, incompatible CORS settings, negative rate limit) | Fix the config value per validation error message                                   |
| **RUV2001** | Middleware execution failed | Tower middleware layer panicked or returned error                                                   | Check custom middleware implementation, verify dependencies                         |
| **RUV2100** | Plugin runtime error        | Plugin runtime could not start or returned invalid protocol data                                    | Check the Node/Bun runtime and plugin setup                                         |
| **RUV2101** | Plugin hook error           | Plugin callback threw or returned an unsupported value                                              | Check the named hook and return `undefined`, `Request`, or `Response` as documented |

---

## Diagnostic Code Ranges

| Range   | Source crate        | Category                      |
| ------- | ------------------- | ----------------------------- |
| RUV10xx | `ruvyxa_graph`      | Route discovery & validation  |
| RUV11xx | `ruvyxa_dev_server` | SSR rendering                 |
| RUV12xx | `ruvyxa_dev_server` | API & server                  |
| RUV13xx | `ruvyxa_dev_server` | Client bundles                |
| RUV14xx | `ruvyxa_dev_server` | Styles                        |
| RUV15xx | `ruvyxa_dev_server` | SSG/ISR/Actions/PPR           |
| RUV16xx | `ruvyxa_dev_server` | Config validation             |
| RUV20xx | `ruvyxa_middleware` | Middleware config & execution |
| RUV21xx | `ruvyxa_middleware` | Plugin bridge                 |

---

## Adding a new diagnostic

Required fields:

1. **Code**: choose next available in correct range.
2. **Title**: concise one-liner describing the violation.
3. **Explanation**: what the contract is and why it was violated.
4. **Span**: file location when known (use `SourceSpan::from_path` if no line/column).
5. **Suggested fix**: concrete, actionable instruction. Use `format!()` for dynamic values.

Example:

```rust
Diagnostic::new(
    "RUV1010",
    "Server directory in client graph",
    format!("File '{}' is inside the server/ directory but is reachable from client code '{}'.", server_file.display(), entry),
    SourceSpan::from_path(&entry),
    format!("Move shared logic out of server/ to a shared module, or keep imports of '{}' only in server/API files.", server_file.display()),
)
```

Add tests for the new diagnostic. If user action is needed for recovery, update `docs/guides/` with
the error code and resolution steps.
