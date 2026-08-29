# Changelog

## v1.1.3 (2026-08-29)

A release about the deployed process as something an operator has to run: taken out of a load
balancer, probed for liveness, held to a deadline, and asked whether the page a reader already holds
is still current. Every suite in this repository asked a fresh server one request and got a correct
answer back, which is why none of what follows was visible. A process is only wrong about draining
while it is being drained, only wrong about a deadline when a render never settles, and only wrong
about a validator when the same reader comes back.

The other half is the shape that has cost the most here — one rule, two implementations, and nothing
holding them together. Two more of those are settled with a fixture each, and a check now fails the
build on a constant declared in both languages that nobody said how to keep in step.

### The socket closed before anything could learn the process was draining

A shutdown signal stopped the listener in the same tick it arrived. A readiness probe answers on a
new connection, so the one answer that tells an orchestrator to stop routing here could not be
reached: by the time the probe was refused, the process it was asking about was already gone.
Everything the load balancer sent while it was still deregistering then failed in a browser instead
of being retried against another instance, which is the exact failure a drain exists to prevent. It
happened on every self-hosted deployment.

The process now keeps listening, and serving normally, for a drain window before it stops accepting
connections. Only the readiness answer changes during it. `RUVYXA_DRAIN_DELAY` sets the window and
defaults to five seconds, capped at half of `RUVYXA_SHUTDOWN_GRACE` so in-flight work keeps a budget
of its own however the two are configured; `RUVYXA_DRAIN_DELAY=0` closes straight away, which is
right where nothing is load-balancing this process. A second signal — Ctrl-C twice — shuts down
immediately.

The test that would have caught this was skipped on Windows, because `child.kill('SIGTERM')` maps
onto `TerminateProcess` there and gives the program no chance to answer anything, and a lane that
runs on one platform is a lane nobody watches. It stopped being run and started being discovered.
The drain test now raises the signal inside the child instead, which runs the listener this
repository owns; signal delivery is the operating system's half and was never what was under test.

### `/__ruvyxa/health`, answered the same way by both hosts

A liveness and readiness probe: `200` with `{"status":"ok","host":…}` while the process is serving,
`503` with `{"status":"draining"}` and `Retry-After: 1` once a shutdown signal has arrived. `GET`
and `HEAD` answer; any other verb is `405` with `Allow`, because a `404` would say the endpoint is
absent rather than that the verb is. It is a reserved framework route on the Axum host and on the
standalone server the adapters emit, and `tests/fixtures/framework-endpoint-conformance.json` holds
the two to the same answers.

It is deliberately incurious: a status and the runtime that answered, nothing else. In-flight counts
and queue depth on a public path are a load oracle for anyone willing to ask often enough.

Those numbers are available, at `/__ruvyxa/metrics`, in Prometheus text exposition — render
concurrency, queue depth, refusals, render timeouts, uptime, and whether a shutdown signal has
arrived. It is served only by the generated standalone server and only when `RUVYXA_METRICS_TOKEN`
is set, compared in constant time; without the token the path answers `404` rather than `401`, so a
deployment that never turned metrics on does not advertise that the path exists. Admission numbers
are absent rather than zero when there is no limiter, because "0 renders running, 0 queued" reads as
a healthy idle process rather than as an unbounded one.

### A render that never settles held its connection for the life of the process

Under `ruvyxa start` a render is a worker round-trip the native host bounds at
`RUVYXA_WORKER_TIMEOUT_MS`, and on a serverless adapter the platform bounds the invocation. The
generated standalone server bounded nothing, so one route that never resolves is a slow leak under
ordinary traffic that ends as an out-of-memory kill with no error in the log.

`RUVYXA_RENDER_TIMEOUT` bounds it, defaulting to the same thirty seconds as the native host so one
project is bounded the same way under `ruvyxa start` and under its own build. A render that passes
it is abandoned with `503` and `Retry-After`. What is bounded is the wait for the `Response`, never
the body behind it — a streamed document and a server-sent-event stream both resolve their
`Response` immediately and then write for as long as they need to. `RUVYXA_RENDER_TIMEOUT=0` turns
it off for a deployment that genuinely renders longer than this and knows it.

### A `413` left the connection reusable

An API route that exceeds its body limit is answered without the rest of the request being read, so
the socket still has a partial body on it. Both hosts sent that `413` on a keep-alive connection:
the client reused it, and its next request was parsed starting from the middle of the body it had
already sent. The symptom is a connection reset on the request _after_ the one that was too large,
which is not the request anybody looks at.

The response now carries `Connection: close` on both hosts. Draining megabytes to keep the
connection warm is the cost the limit exists to avoid.

### A stored document was re-sent in full on every navigation

`DOCUMENT_CACHE_CONTROL` tells a browser to revalidate before every reuse, and a revalidation with
no validator to offer can only be answered with the whole document again. Every SSG, CSR, and ISR
document was therefore re-sent in full to a reader who already had it, on both hosts.

Those three strategies now carry a weak `ETag`, and an `If-None-Match` that matches is answered
`304`. SSR and PPR are deliberately absent: their document is produced for this request, may carry
one visitor's data, may still be streaming, and is `no-store` either way, so there is nothing for a
validator to be about. `document_has_validator` in `ruvyxa_graph` is the single answer to which
strategies those are, and `tests/fixtures/deploy-output-conformance.json` replays both halves.

The two hosts deliberately do **not** share the validator's value — the native one hashes with
blake3 and the deployed one with SHA-256. A validator is opaque and scoped to the origin that issued
it, and no client ever holds one from both.

### Both hosts send one `Cache-Control` for a document

`document_cache_control` lived in the CLI crate, so it described what a deployed function would send
and the dev server sent nothing of the kind. The same route was cached one way under `ruvyxa start`
and another way once deployed — a difference that only appears on the second request, which is not
the one a parity check makes.

It moved to `ruvyxa_graph`, where both callers can own it, and the render pipeline applies it. The
conformance test moved with the function rather than being copied.

### A deployed server trusted a header nothing was writing

A deployed function has no transport peer to weigh, so the previous rule was that it treats its
platform ingress as trusted by construction and reads `cf-connecting-ip`, `x-vercel-forwarded-for`,
and `true-client-ip` first. That is right on Cloudflare and on Vercel, where the ingress writes
those headers and overwrites what a caller sent. It is wrong on the standalone server the `node`,
`bun`, `deno`, `aws`, `railway`, and `render` adapters emit, which is an ordinary HTTP server bound
to `0.0.0.0` with nothing in front of it: the header was simply whatever the caller typed, so one
client rotating it got a fresh rate-limit bucket per request.

The declaration is per-adapter now, through `createHandler`'s `clientIpHeaders`, and an adapter that
declares none weighs `X-Forwarded-For` exactly the way the native host does — against the trusted
list, taking the rightmost hop that is not itself a trusted proxy.
`tests/fixtures/client-ip-conformance.json` carries the cases.

### Two rules, two implementations, and now a fixture for each

**The CSS Module class name.** The stylesheet comes from `crates/ruvyxa_bundler`, and the class map
the server renderer imports comes from `packages/ruvyxa/runtime/compiler.mjs`. Both fold the
project-relative path before hashing it, so that a case-insensitive filesystem cannot produce two
hashes for one file — and they folded differently. JavaScript used `String.prototype.toLowerCase`,
which is the full Unicode default case conversion; Rust used `str::to_ascii_lowercase`, which leaves
every non-ASCII letter alone. On a case-insensitive filesystem `Ü/card.module.css` and
`ü/card.module.css` are one file, and Rust hashed them as two.

A project with any non-ASCII uppercase letter in the path to a `.module.css` file therefore rendered
with a class no rule matched. The page is not broken in any way a build or a log can report; the
styles are simply absent. The collision guard that exists to catch exactly this under-reported for
the same reason. Full Unicode folding is the shared behavior now, because it is what the guard
already claims and what a case-insensitive filesystem actually performs, and
`tests/fixtures/css-module-class-conformance.json` holds both.

**The locale redirect.** An unprefixed URL cannot match a `/[lang]/…` route, so both hosts redirect
it to a prefixed one — `crates/ruvyxa_dev_server/src/i18n.rs` for `dev`, `start`, and `preview`, and
`packages/ruvyxa/runtime/serverless-handler.mjs` for every serverless adapter. They disagreed twice,
and both were invisible because every test on both sides set `detectLocale: true` and asked for an
ordinary path:

- `detectLocale: false` turned the redirect off entirely in the deployed handler and turned only
  _detection_ off in the native server. The option says which signals pick the locale, not whether
  locale routing happens — so with it off, `/about` reached `/en/about` under `ruvyxa dev` and 404ed
  once the project shipped, for every unprefixed URL.
- The reserved namespace was excluded by raw byte prefix in the deployed handler and by whole
  segment in the native server, so `/__ruvyxa-notes` — a page a project may legitimately own — was
  redirected in development and silently excluded once deployed.

The native server's behavior is the shared one in both, and
`tests/fixtures/i18n-routing-conformance.json` is replayed by a Rust test and a JavaScript one.

**And the constants nobody had registered.** `scripts/check-cross-language-constants.mjs` enumerates
every `SCREAMING_SNAKE` constant declared in both languages and requires each name to say what holds
the two together: a shared fixture, a cross-language test, a scalar this script normalizes and
compares itself, or an explicit note that the name means two unrelated things. A pair registered
nowhere fails the build. It does not diff every value, because the same fact is legitimately encoded
differently in the two languages — `&["tsx"]` against `['.tsx']`, `52_428_800` against
`50 * 1024 * 1024` — and a blanket comparison would be noise rather than a gate. Twenty-nine pairs
are held today. It runs in `pnpm release:validate` and on its own as
`pnpm check:cross-language-constants`.

### `revalidatePath()` reaches what a visitor is served, or says that it cannot

**Netlify dropped two other pages with it.** A cache tag list is comma-separated, so a path has to
become one token, and the mapping has to be injective — which folding every run of non-alphanumerics
to a single `-` is not. `/a/b`, `/a-b`, and `/a_b` all produced `ruvyxa-a-b`, so revalidating one
dropped the other two from the edge. The tag is a SHA-256 digest now: one token, the same length
whatever the path, and unable to collide by construction.

**Vercel purged the wrong domain.** The origin a purge is sent to is read from the request being
served, so that a preview deployment purges its own domain and a custom domain purges its own. It
was held in a module-level variable, and one instance of the function answers more than one request
at a time — so a concurrent request overwrote it, and the purge went to the other request's domain
while the page it was asked to drop stayed cached on the domain a visitor was actually reading. It
is an `AsyncLocalStorage` now, and the handler runs inside a request-scoped context.

**And the platforms where it cannot work say so at build time.** `onDemandRevalidation` joins
`onDemandImages` in `tests/fixtures/adapter-contract.json`, per adapter. Where a cache sits in front
of the function and the adapter has no way to drop one path from it — Firebase Hosting and Amplify's
CloudFront both cache on the `s-maxage` the handler sends — a project with revalidating routes is
warned during `ruvyxa build`. The forced write still lands in the function's own store; what it
cannot do is drop the copy the platform is serving, so the page keeps answering with the document it
had until the window expires on its own. That is a correct deployment, just not the one the call
implies.

### Netlify writes `netlify.toml` by default

`projectConfig` now defaults to `true`, and `frameworksApi` defaults to `!projectConfig`.

The publish directory is the one question Netlify cannot learn from the build: the Frameworks API
has no key for it, and a plugin cannot supply it without a `netlify.toml` of its own — Netlify
installs a plugin from its own UI only when the plugin is listed in Netlify's directory, and a
community package has to be declared in `[[plugins]]` and installed as a devDependency. So the
choice was never "config file or no config file"; it was "one generated file, or the same file
written by hand".

The two are alternatives rather than layers, which is why turning one on turns the other off.
Shipping both puts two functions on the site — the one under `.netlify/v1/functions` and the one the
`netlify.toml` `functions` directory declares — and both claim `path: '/*'`, so which of them
answers a request is not a question with an answer.

Netlify reads `netlify.toml` _before_ it runs the build command, so the file has to be committed to
take effect. The build writes it once and never overwrites an existing one.

### Deno Deploy: no `deno.json` in the deploy directory

The Deno adapter emitted one, and a `deno.json` makes the directory it sits in Deno's own
configuration scope. That scope declared no dependencies, so the npm packages a server-components
render reaches for at request time stopped resolving and every such route answered `500`. Without
the file, Deno walks up to the project and resolves them. The adapter's README says why, and names
the entrypoint to give the platform.

### Cloudflare Workers Builds and Deno Deploy are detected

`WORKERS_CI` and `DENO_DEPLOY` join the platform environment variables the build reads, so neither
needs an adapter named in `ruvyxa.config.ts`. `ARCHITECTURE.md` documents all eight, and says which
of them is Cloudflare Pages and which is Workers.

### Vercel image widths are snapped rather than forwarded

Vercel answers `400` for any `?w=` its `images.sizes` did not declare, and `<Image>` writes the
author's own `width` into the `srcset` without snapping it — so forwarding the requested width
verbatim broke exactly the images that render correctly under `ruvyxa start`. A request is widened
to the nearest declared size instead, which is the size Vercel would have served anyway. The widths
are computed once and threaded into the emitted handler rather than hardcoded in it, so the declared
list and the code that snaps to it cannot disagree.

### The dev server is exercised end to end, and one flaky test is not flaky

Every other end-to-end lane here exercises a build, and the Rust suites cover the dev server's
pieces — the watcher, the HMR tracker, the router, the render cache — one at a time. Nothing started
the command developers run all day and watched an edit travel from the filesystem to the socket, so
the seam between those pieces was covered by nobody: a watcher that stops emitting, a tracker that
classifies an edit wrongly, a sequence number that stops increasing, and a socket that never sends
anything at all all leave every unit test green.

`scripts/smoke-dev-server.mjs` runs the real command against a real project, edits files under it,
and checks what the browser would have been told. Every edit is reverted in a `finally`, and the run
fails if a source file is not byte-identical afterwards — a smoke test that leaves the tree dirty is
a smoke test nobody will run twice. It runs in CI, alongside the full project flow on Windows.

Separately, a linker test asserted `is_empty()` over a process-global collector while a sibling test
recorded into it from another thread. It reproduced roughly once in seventy-five runs on sixteen
threads, which is why it only ever appeared under the CPU pressure of a workspace-wide run. It links
a specifier no other test links now, and asserts on that key alone.

## v1.1.2 (2026-08-26)

A release about answers that were wrong and looked right. A read that fails says something the
caller cannot reconstruct, and three places turned that failure into a default value that was also a
legitimate answer — so a route's Flight export was recorded as absent on every build run from
outside the project directory, a damaged route manifest was rewritten with no routes in it, and an
unreadable route file was reported to the browser as a route with no payload. Each one left a green
build behind it. The shape has a gate now.

The rest is the same question asked of things that were never checked: whether a warning is still
true after the adapters it describes changed, whether a diagnostic carries enough to act on, whether
a cached artifact still reports what the build that produced it reported, and what a static analyser
makes of code nobody had pointed one at.

### Breaking: `notFound` from `ruvyxa/server` is replaced by `status(code, message?)`

Two public exports were called `notFound`, and they did opposite things:

| Import                          | Behaviour                                           |
| ------------------------------- | --------------------------------------------------- |
| `notFound` from `ruvyxa/server` | **returned** a 404 `Response`                       |
| `notFound` from `@ruvyxa/react` | **threw** a tagged signal to render `not-found.tsx` |

Neither failed at the import, which is where a name collision does its damage. A page that took the
server half rendered a `Response` object where React expected an element; an `app/api/` handler that
took the browser half threw instead of answering. Both type-check at their own call site, so nothing
caught it — the documentation carried a "do not confuse it with" note and a troubleshooting entry
instead, which is the state a name is in just before somebody loses an afternoon to it.

The **throwing** one keeps the name: it matches Next.js, and it is the one a page wants.

The server half is not merely renamed. It was the only status with a helper at all — every 401, 403,
409, and 422 was being written out as `new Response(message, { status })` by hand — so the 404-only
helper became a general one, which is also how it ends up shorter than the name it replaced:

```ts
// app/api/post/route.ts
import { json, status } from 'ruvyxa/server'

export function GET() {
  if (!user) return status(403, 'Forbidden')
  if (!post) return status(404)
  return json(post)
}
```

```tsx
// app/blog/[slug]/page.tsx — unchanged
import { notFound } from '@ruvyxa/react'
if (!post) notFound()
```

`status` validates what `Response` would have rejected later and with a worse message: a code
outside 200–599 throws naming the helper, and a body on 204, 205, or 304 is refused rather than
producing the platform's `Response with null body status cannot have body`.

**Migration:** `notFound()` → `status(404)`, and `notFound(message)` → `status(404, message)`,
wherever it came from `ruvyxa`, `ruvyxa/server`, or `@ruvyxa/core`. Imports from `@ruvyxa/react` do
not change. No alias is kept — one would leave the collision in place, which is the whole defect.
Note that `status(404)` with no message now has an **empty body** where `notFound()` defaulted to
the text `Not found`; pass the message explicitly to keep it.

The rule is now a test rather than a paragraph: `tests/packages/core/entry-point-collisions.test.ts`
loads every built entry point of the server half and the browser half and fails on any name
reachable from both. It is verified to fail when the old `notFound` is restored.

> `redirect` is the same shape waiting to happen — `ruvyxa/server`'s returns a `Response` while
> Next.js's `redirect` throws. Ruvyxa exports no browser-side `redirect` today, so there is no
> collision to fix; the new gate will fail the day one is added under that name.

### A route's `flight` export was recorded as absent on every `--root` build

`ClientBundle::entry` is the route file as the published manifest carries it — project-relative,
because an absolute build-machine path in `entry` would make two machines emit different bytes. The
build then read that path to record whether the route exports `flight(context)` and whether it
declares `'use cache'`, and read it with `fs::read_to_string` — which resolves against the process
working directory, not the project.

So `ruvyxa build --root <somewhere-else>` read nothing at all, and `unwrap_or_default()` turned that
into `""`. An empty source exports no `flight` and declares no `'use cache'`, which are perfectly
ordinary answers, so nothing downstream could tell the two apart. Every route in the shipped
`route-manifest.json` said `flight: false`; the browser router stopped requesting payloads for
routes that do produce them and fell back to a full document load, and `RUV1842` — the guard that
refuses `'use cache'` on a route with no Flight producer — could not fire at all.

Running `ruvyxa build` from inside the project directory was correct, which is how it stayed
invisible: that is how it is run by hand, and `--root` is how CI, monorepos, and every scripted
build run it. No fixture in the repository exported `flight`, so nothing looked either. There is one
now, and it builds through a project root that is deliberately not the test's working directory.

The same read sat behind `/__ruvyxa/flight` and the development route table. An unreadable route
file was answered there as `501 This route does not expose a Flight payload` — the wrong cause,
named confidently, with nothing logged. Both callers now share one reader, `route_module_facts`,
which reports the failure instead of defaulting: the endpoint answers 500 and logs the path, and the
route table leaves the route out rather than advertising it as having no payload.

### A damaged route manifest was replaced by one naming no routes

`write_style_asset` reads `client/route-manifest.json` to add the stylesheet URL and writes the
whole document back. It parsed that file with a default of `{"routes": []}` — so a manifest that
failed to parse was not merely mis-read, it was **overwritten** with one naming no routes at all.
That file is what every host reads to find a route's scripts: the Rust server, the generated
standalone server, and each adapter's function bundle.

The build still succeeded. The first symptom was a deployed site whose client router knew no routes
and answered every navigation with a full document load.

No corruption is needed to reach it: a build interrupted mid-write leaves a partial file, and so do
two builds sharing one output directory. The parse failure is now reported, and names the file to
delete and rebuild.

### `check:silent-defaults` — a failed read may no longer become a value

The two defects above are one shape, and it had shipped three times: a read whose failure is turned
into a default that is **also a legitimate answer**, so nothing downstream can distinguish them.
That is not a lost log line; it is a wrong result that looks right.

`scripts/check-silent-defaults.mjs` fails the build on `unwrap_or_default()` and
`unwrap_or_else(|_| …)` applied to a read or a decode — `read_to_string`, `fs::read`, `from_str`,
`from_slice`, `from_utf8`, `.parse()` — outside an allowlist that carries the reason for each
accepted site, and fails just as loudly when an allowlist entry stops matching anything. `.ok()` and
`.ok()?` are deliberately not flagged: they hand the caller `None`, which is an honest "no answer"
it can branch on, and is what a cache lookup wants.

It runs in `pnpm release:validate` and on its own as `pnpm check:silent-defaults`.

### `.env` is a build input, and the compile cache now knows it

`import.meta.env` is substituted into every module the compiler emits, as a frozen literal — that
substitution is what makes a `RUVYXA_PUBLIC_*` value readable in a browser at all. So the value is
_in_ the browser bundle, and the caches that hold that bundle were keyed on the configuration alone.

Editing `.env` and rebuilding produced a build whose pre-rendered HTML carried the new value —
`prerender_context_hash` has always keyed on the environment — and whose browser bundle still
carried the old one, served from the compile cache. One build, two answers for the same variable in
the same page, and the browser's is the one that wins the moment hydration runs.

`config_dependency_hash` is now `build_dependency_hash`, named for the build rather than the config
because the environment is the half that was missing, and it folds the project environment in
alongside the configuration. The whole environment goes in, not only the public names: over-keying
costs a rebuild that reproduces identical bytes, while under-keying serves a bundle built from an
environment the project no longer has. A project with no readable `.env` folds in an empty map.

The two halves are joined by a NUL rather than a space, so an environment value containing the
separator cannot be rearranged into another project's hash.

### On-demand image optimization is adapter-conditional, and now says so

`image.onDemand` is served at `/__ruvyxa/image` by the Axum host, which resizes through the Rust
image pipeline. No build output carries that pipeline — but an adapter may supply its own, and two
do: `@ruvyxa/adapter-vercel` and `@ruvyxa/adapter-cloudflare` each hand `createHandler` an
`optimizeImage` that forwards to their platform's optimizer.

The capability contract had no way to express "depends on the adapter", so it recorded the
unconditional answer and every consumer repeated it: `ruvyxa build` warned a working Vercel
deployment that its responsive images answered 404, `ruvyxa test:parity` agreed, and the guide said
the same in both languages — while the deployment matrix two chapters away already said
"adapter-dependent".

`tests/fixtures/adapter-contract.json` carries `onDemandImages` per adapter now, and it is the only
place the answer is written: `ruvyxa build`'s warning reads it, `ruvyxa test:parity` resolves the
project's configured adapter against it, and `scripts/sync-adapters.mjs` renders it as a column of
the adapter matrix in both language guides. A test holds the table to the adapter sources that
actually decide it, so an adapter that gains or loses an optimizer fails the build rather than
drifting.

The warning stays silent when the adapter cannot be named — an inline `adapter: vercelAdapter({…})`
is an object rather than a name, and a third-party package is not in the table at all. A wrong
answer errs in one direction here: saying nothing costs one full-size download, while telling a
working deployment it is broken sends the reader to turn off a feature that works.

`image.quality` also reaches deployed builds now. It decides what a request without a `q` parameter
is encoded at, and the deployed handler had a hardcoded `82` and read no project value, because the
build published `onDemand` and `maxWidth` into the runtime policy and not the quality beside them.
`tests/fixtures/dynamic-image-conformance.json` holds the endpoint's bounds and defaults across both
hosts.

### Diagnostics carry their code, their message, and the line they came from

A parser failure reported a **count**. "3 parse errors" is a number a reader can do nothing with,
and it arrived after a full build rather than at the file that could not be parsed.
`describe_parse_diagnostics` extracts Oxc's own messages now, and `parse_diagnostic_line` maps each
diagnostic's byte offset to a 1-based line, so the report names what is wrong and where.

Codes and messages were being joined by hand at every site that emitted one, and the spellings had
drifted apart: one path printed a code twice (`RUV1700 RUV1863`), another defaulted a missing code
to the empty string and printed a message with a leading space. `label_with_code()` in
`ruvyxa_diagnostics` owns that formatting now, and a test walks every crate to fail a hand-written
positional join — discovering the crates rather than naming three of them, and blanking comment
lines rather than dropping them so the reported line numbers stay true.

The capability parity table reads in one vocabulary across both columns, and its columns are named
`ruvyxa start` and `deployed` rather than `native` and `deploy`, which is what the commands are
called.

A missing deploy adapter is also resolved before the build starts rather than after it: a name that
cannot be resolved fails immediately instead of after every route has been compiled.

### Boundary warnings no longer vanish on a cache hit

A non-fatal boundary diagnostic — `RUV1008`, a private `process.env` read reachable from browser
code — was reported where the bundler ran, and the bundler does not run on an artifact-cache hit. So
the warning printed on the first build of a project and on no build after it. A warning that is a
function of cache state rather than of the code is the one thing a warning must never be.

Diagnostics are carried on `ClientBundle` and stored with the cached artifact, so a cache hit
restores them. They are stored as rendered strings, because `Diagnostic::code` is a `&'static str`
and cannot be deserialized from a cache file.

### The prerender cache sees plugin head entries, and the worker file list is checked

A plugin that contributes `<head>` content changed what a pre-rendered page contains and nothing in
the prerender context hash changed with it, so an edited plugin head served the previous page from
cache. The plugin head content hash is part of that context now, and document head composition is
one `prerender_head` function shared by pre-rendered pages, the not-found page, and the deployment
manifest, rather than three inline constructions that could disagree.
`tests/fixtures/document-head-conformance.json` holds pre-rendered and runtime-rendered documents to
the same head.

`WORKER_RUNTIME_FILES` is a hand-maintained list of the runtime modules a worker bundle must carry,
and nothing checked it against the imports the worker actually has. Two tests do now: one walks the
worker's import closure through the bundler's own AST parser and fails on a file the list omits, and
one fails on a list entry naming a file that does not exist.

### CodeQL, and what it found

A CodeQL workflow analyses Rust, JavaScript/TypeScript, and the workflow files themselves, on every
push to `main`, on tags, on pull requests, and weekly — the schedule so a newly published query
reaches the repository without waiting for someone to open a pull request. It runs the
`security-extended` pack, and the toolchain and action versions are pinned so a scan is
reproducible.

Three findings in shipped code were real and are fixed:

- **A matched file's name was built into generated source.** `import.meta.glob` embeds each matched
  specifier into the module it returns, encoded with `JSON.stringify` — which is a JSON escaper. It
  escapes the quote and the backslash and is right about the rest, except for U+2028 and U+2029: it
  emits both literally, and a JavaScript string literal has only accepted them since ES2019, so on
  an older parser they end the literal and everything after the file name is code. A globbed
  directory is writable by any dependency that can put a file in it. A specifier carrying a control
  character or either separator is now refused, naming the path, rather than escaped — a file name
  that cannot be written into source verbatim is one nobody meant to import, and rewriting it
  silently would leave a module whose key does not match the file on disk.
- **Two path normalizations backtracked.** `projectRelativeOutDir` and the font `publicPath`
  normalizer each stripped separators with a regular expression the engine retries from every start
  position, so a value with many `/` costs time quadratic in its length. Both are configuration
  rather than request input, so neither was reachable from outside; both are single-pass scans now.
- **Three test assertions could not see an upper-case tag.** Assertions that a `<script>` was
  neutralized were case-sensitive, so they would have passed against an escaper that stopped
  lowercasing.

The remainder are recorded on the alerts with the reason: fixture nonces inside `#[cfg(test)]`,
containment assertions that are correct while unanchored, a backslash escaped on the line above the
one flagged, and the first-`*` substitution that a TypeScript `paths` mapping specifies.

Separately, a bare specifier the browser graph cannot resolve is now reported at the end of a build
rather than only stubbed. The stub throws at the first interaction with `RUV1611`, and the build it
came from was green — which on a hosting platform, where the install differs from the one on the
developer's machine, is the whole distance between a working site and a blank one. Every importer of
each missing package is named.

### Concurrent test suites truncated a shared helper

`pnpm -r test` runs suites in parallel and each one invoked `tsc` into the same `.test-build/`
directory, so two compilations could truncate a shared helper mid-write. The failure surfaced on CI
as `SyntaxError: does not provide an export named …`, which describes neither the cause nor the
file. Each suite compiles into its own `.test-build-<suite>/` now, passed to `tsc` on the command
line so no package has to restate it.

## v1.1.1 (2026-08-25)

A release about the half of the framework nothing was asking questions of. Every adapter had unit
tests over the files it emitted; four of the eleven had never had one of those deployments _started_
and asked anything. Extending the deployment smoke from four lanes to all eleven, and then pointing
it at the feature fixture instead of the small one, is what found most of what follows — and every
one of those defects shipped in v1.1.0 with a green CI.

### A pre-rendered page lost every security header on four platforms

`DEFAULT_SECURITY_HEADERS` — seven of them, including `X-Frame-Options: DENY` — is attached by
`createHandler` and by the standalone server. Both of those are **the function**. But every one of
these platforms answers a pre-rendered document and every public file **from its own edge, without
invoking the function at all**: Vercel's `handle: filesystem`, Netlify's publish directory,
Amplify's `Static` route target, Firebase Hosting's `public`.

So a page that is frame-denied under `ruvyxa start` was framable the moment it was pre-rendered and
deployed, on `vercel`, `netlify`, `aws`, and `firebase` at once. Every check stayed green while it
was true, because the status was 200 and the markup was right.

`cloudflare` and `static` were never affected: both write a `_headers` file through
`headersFileContents()`, which is where the pattern already existed. The other four had no such
mechanism, and each needed a different one:

- **vercel** — a `{ src: '/(.*)', headers, continue: true }` route, first, ahead of
  `handle: filesystem`. `continue` attaches headers without changing where the response comes from.
- **netlify** — a `{ for: '/*', values }` rule, first. Its header rules stopped being
  `{ for, cacheControl }` and became `{ for, values }`, feeding all three outputs it writes.
- **firebase** — a `{ source: '**' }` entry, first in `hosting.headers`.
- **aws** — **`customHttp.yml`**, because an Amplify route target carries `cacheControl` and nothing
  else. Written at project scope with `skipIfExists`, so a project that already keeps its own
  Amplify header rules there keeps them.

The smoke lane for each of the four parses the asset headers **out of the config that adapter
emitted**, in the order that platform applies them, rather than restating the expected value — which
would have proved only that the check and the script agree.

### A deployed build never streamed

A route with `Suspense` boundaries at 300ms and 1200ms sent its first byte at **1224ms** on a
deployed build, and at 735ms under `ruvyxa start`. The page's own documentation promised the shell
arrives before either section renders. On every deployment it did not: the whole document was
buffered to a string first.

The buffering was deliberate — it is how a deployed function survives a `<Suspense>` child that
rejects _after_ the shell has gone out, which used to answer 500 in production while the browser
logged only `Uncaught Error: Connection closed`. What actually blocked streaming was three
whole-document string transforms that ran after the render: the asset injection, the `lang`
attribute, and the `[locale]` rewrite.

Those were split so the two ends can be placed separately, and the document is now assembled around
a stream: the shell is accumulated until it carries a `</head>` or a `<body`, everything after that
passes through, and the tail is placed before the closing `</body>` React writes last. The tail is
awaited rather than passed as a value, because a server-components payload is complete only when the
Flight render is — long after the first bytes have left, and the browser needs it before hydration
rather than before the first paint.

`renderServerComponentsStream` already existed and was used only by the native host; the generated
route registry uses it too now. The plain path uses `renderToReadableStream` directly, whose promise
rejects exactly when the _shell_ failed — so the tolerate-after-shell policy the buffered renderer
implemented by hand comes for free.

**Streaming is per-request and opt-in, and only the plain SSR path asks for it.** Pre-rendering and
the ISR write need the finished string; so does the `requestScoped` check that guards them, whose
answer at return time would be a lie when a `Suspense` child reads cookies after the shell has gone
out; and the `[locale]` rewrite needs the whole `<html>` tag. Nothing that is stored streams.

First byte on that route is now **32ms**.

### `HEAD` on a route was refused, and no `405` said what to use instead

A `route.ts` exporting only `GET` answered `HEAD` with `405`. RFC 9110 §9.3.2 makes `HEAD` identical
to `GET` without the content, so a resource that serves one serves the other — and `HEAD` is what an
uptime monitor, a link checker, and a CDN revalidation send first. And no `405` from any host
carried an `Allow` header, which RFC 9110 §15.5.6 says it MUST: the caller learned its method was
wrong and never which one to use, and a CORS preflight had nothing to read.

Both were true in all three hosts that dispatch API routes, because each had its own copy of
`mod[method]` followed by a bare `405`. They agreed with each other and were wrong the same way,
which is what a rule copied three times does. `runtime/api-methods.mjs` is now the one place, and
its test greps the three hosts for a re-introduced copy.

### A plugin that adds a header made every `204` and `304` a `500`

The response hook every page of the documentation shows rebuilds the response —
`new Response(response.body, { status, headers })` — because `Response.headers` is immutable. That
throws for a status the fetch specification forbids a body on, unless the body is exactly `null`.

The native host encoded "no body" as an **empty string** when it handed a response to the plugin
runner, and an empty string is not `null`. So a project with any `http.onResponse` registration
answered 500 for every 204, 205, and 304 it produced. Both example plugins in the demo use exactly
that shape.

The predicate now lives in `runtime/plugin-http.mjs` rather than beside its caller, so a test can
reach it — the caller's module speaks NDJSON over stdio and importing it starts a plugin runner.

### A request with a body became a `500` wherever a response hook was registered

A hook is handed a clone of the request, so that reading the body cannot take it from the route
handler that needs it next. But a `Request` whose body has already been consumed cannot be cloned at
all — `clone()` throws `TypeError: unusable` — and by the time the _response_ hooks run, the handler
has usually consumed it. The route ran, produced its answer, and the response stage threw it away.

A used body is gone either way, so the fallback hands the hook the same URL, method, and headers
with no body rather than failing.

### An oversized request body poisoned the next request on the connection

The point of `security.apiLimit` is not to read the rest of the body, so the rest is still in flight
on the socket when the refusal goes out. Reusing that connection reads those bytes as the beginning
of the next request — so a client that pools connections, which is every browser and `fetch` itself,
saw a _later, unrelated_ request die with `ECONNRESET`. The connection is retired now, which is what
RFC 9110 asks of a server that answers before the body is read.

### `import.meta.env` was `{}` in a deployed server render

A build reads `.env` and `.env.local` itself and hands the values to the processes that need them;
it does not put them in its own environment. Substituting from `std::env` alone therefore produced
`Object.freeze({})` for every project that keeps its public values in a file — which is all of them.
The host now records the project environment it loaded and the compiler substitutes from that.

`import.meta.env` also had no TypeScript type despite being documented.
`packages/ruvyxa/types/env.d.ts` declares it, referenced from all three type entry points — a new
ambient file reachable from only one of them is absent exactly where the reader is.

### Cloudflare gained ISR and PPR

The blocker was never architectural: `readPrerendered` and `writePrerendered` are adapter
capabilities, not filesystem calls. The blocker was that `readPrerendered` was **synchronous** and a
Workers KV read returns a promise. The handler awaits it now — awaiting a non-promise costs a
microtask and leaves every filesystem adapter unchanged — and
`cloudflare({ isr: { kvBinding: 'RUVYXA_ISR' } })` wires KV with a freshness stamp and a retention
multiple, so a stale document survives long enough to be served while it refreshes.

**The capability follows the option.** With no binding the adapter declares no `isr` or `ppr`,
because a Worker that re-renders every request while reporting `x-ruvyxa-isr: HIT` is worse than a
build that stops.

### `export const runtime = 'edge'`, and a Vercel function that can act on it

A page or `route.ts` may declare `runtime`. It is parsed from the route's own file — never from a
dependency, which cannot move the route that uses it — and `validate_app` then walks the route's
graph plus its layout chain and refuses any import of a Node built-in a V8 isolate lacks, with
**`RUV1013`** naming the module and the specifier. An unrecognised value is **`RUV1012`**, never a
silent fall back to Node.

The declaration is an **API-surface constraint the build enforces**, not a promise about which
datacentre answers the request: a Node host answering an edge route is always correct, because every
API an edge route may use exists in Node too.

`vercel({ splitEdgeRoutes: true })` acts on it, emitting a second function whose registry is
compiled for the edge and which carries only those routes. Eligibility is refused by name
(**`RUV2203`**) rather than silently downgraded: a route declaring `edge` that renders server
components, or uses ISR or PPR, is rejected, because server actions, RSC, and Flight are answered
through single paths owned by one function. The Node function deliberately keeps every route,
including the edge ones — it is the catch-all, and a pattern is not the router.

Two additive extensions made it possible, and both default to today's behaviour, so the other ten
adapters are untouched: a `function` artifact may name its own bundle `target`, and may name the
`routes` it answers.

### A non-ASCII identifier before a `/` blanked the rest of the line

The source scanner walks bytes, and JavaScript identifiers are Unicode. A non-ASCII character
standing where a token ends is the _tail_ of one, so `café / 2` is a division — but the walk read
the `/` as opening a regular expression and blanked everything to the next `/` on that line. An
`import` there stopped being a dependency edge; in the minifier a `process.env.NODE_ENV` guard
stopped being folded, which ships development-only code to the browser. A minified dependency is one
long line, so the newline that stops a runaway literal never arrives.

Three more shapes of the same class went with it:

- **A regex inside a template interpolation, containing a quote.** An interpolation is code, so it
  can hold a regex, and a regex can hold a quote. Without tracking that, the `'` in
  `` `'${value.replace(/'/g, "''")}'` `` opened a string that ran to the next quote and every
  literal and comment after it in the file was read inside out. `js-yaml` ships exactly this line.
- **A string line continuation.** A backslash at the end of a line inside a string continues it; the
  walk ended the string there and read the following text as code, **inventing** import edges and
  private-environment reads that the file does not contain.
- **A decorator on the same line as the member it decorates.** The rule required `@` to start its
  own line, so `class Svc { @log run() {} }` was reported as decorator-free, the stripper returned
  the source untouched, and the `@` reached the emitted bundle where it is a syntax error. That is
  the shape a formatter picks for a short member and the shape every minified dependency has.

A leading `#!` shebang is stripped rather than parsed.

Both scanners are held to `tests/fixtures/source-scanner-conformance.json`, now 15 cases.

### `export { source as from }` dropped the export

`from` is a keyword only where a specifier follows it, and it is also an ordinary binding name:
`export { source as from }` renames a binding _to_ `from`. Entering the re-export branch on the
keyword and bailing out of it on the missing specifier meant the declaration branch was never
reached, and the export was dropped **with no diagnostic** — the importer saw `undefined`. The
specifier decides now, on both sides.

Top-level `await` in the client graph builds rather than failing with a message that named nothing.

### A relative specifier that climbs a directory was never resolved

`../` was not stripped when a linked bundle resolved a specifier by path, so an import that climbs
out of its directory reported `RUV1612` — cold-fail, warm-pass, which is the worst shape a cache bug
takes. `date-fns` is one of the packages that does this. The `RUV1612` message also asserted the
wrong cause; it names the unresolved specifier now.

Linked bundles are syntax-validated as part of the build rather than at the moment something tries
to run them.

### The two compilers disagreed about which dialect a file is

`.ts` is not JSX — `<T>value` is a type assertion there — and a `.js` file may contain JSX. Each
compiler had its own answer and the two differed in both directions, so a file that built in one
lane failed in the other. `tests/fixtures/module-kind-conformance.json` now carries a
`parserDialect` section that both replay.

### A non-ASCII identifier was truncated in the browser lane

Nine places used `char::is_alphanumeric()` as a JavaScript identifier test. It is not one: a Thai or
Devanagari name is cut at its first combining mark, so the client bundle declared one name and
referenced another and hydration died. All nine go through the Unicode identifier tables now.

### Diagnostics for gaps that used to be silent

- **A plugin that rewrites a module the server renders unchanged.** `build.onTransform` runs in the
  browser compile only; the server render reads the same file through the other compiler, which has
  no plugin host to ask. A rewritten value therefore appears in one half of a document and not the
  other. Reported at the first moment both halves are known.
- **A route that server-renders and cannot hydrate.** Named rather than left as a page whose buttons
  do nothing.
- **A capability no deployment can serve.** `realtime@1` and `presence@1` need a process that stays
  alive, so `ruvyxa build` refuses every serverless adapter with `RUV3201` and `test:parity` reports
  it — but both arrive after the application has been written around the transport. `ruvyxa dev` now
  prints the capability and its path on startup, gated on `watch`, because `ruvyxa start` genuinely
  does serve them.
- **A virtual module id from a resolve hook.** `'\0virtual:x'` and `'virtual:x'` were each joined
  onto the project root and handed to the filesystem, surfacing as
  `strings passed to WinAPI cannot contain NULs` and `The system cannot find the file specified`,
  naming no plugin. Both fail with a diagnostic naming the plugin, the specifier, and the path shape
  to return instead. The contract is now in the plugin documentation, which never stated it.
- **A server-components route whose `error.tsx` only runs on the server.** On such a route the
  browser bundle contains client components and nothing else, so a special file without
  `'use client'` cannot be in it. The author saw their error page when the failure happened during
  the server render and the framework's built-in message when it happened in the browser — two
  different error pages for one route, decided by where the error occurred. Named now.

An API handler may return data instead of a `Response`, which is sent as `Response.json(value)`.
That convenience existed and was undocumented, so a handler that returned the wrong thing answered
`200` and looked deliberate; the documentation now says so, and the two cases that stop the request
with `RUV1504` — returning nothing, and returning something JSON cannot serialise — say which
handler and why.

### A native binary from another release could load this release's JavaScript

The two halves are not independent: the Rust CLI resolves `runtime/*.mjs` **by path** out of the
installed `ruvyxa` package, and the contracts between them hold only within one version.
`optionalDependencies` carried `workspace:^`, which publishes as a caret range and matches a later
minor — so the combination was reachable from the registry, and neither side could tell. The binary
now refuses a version that does not match the package running it, and says why.

`.github/workflows/*.yml` is parsed by a test rather than matched as text. Every assertion in it
exists because the property was broken once and nothing noticed: a release published five native
packages and then failed, a commit on `main` was cancelled and shipped inside a tag with no verdict,
and the whole supply chain hung off mutable action tags.

### How a deployment is proven

The deployment smoke used to build and launch **four** of the eleven adapters. It now builds and
launches **all eleven**, in four transport shapes, because what the adapters do not share is exactly
what had never run: the request translation each one hand-writes, and the static half its platform
answers without ever calling the function.

- `node`, `bun`, `deno`, `railway`, `render` emit a **program** and are spawned.
- `aws` emits a program **behind a CDN**, so it is spawned on an inner port with the platform's
  static half in front of it.
- `cloudflare` and `netlify` emit a **fetch module**.
- `vercel` and `firebase` emit a **Node request handler**, called with the real `(req, res)`.
- `static` emits **no server at all** — the deployment is the publish directory, and it is the only
  lane that can check `_headers` and the `404.html` convention, which is the only way a project's
  own not-found page is reachable with nothing running to read a manifest.

`railway` and `render` emit the node adapter's server verbatim and still get their own lanes,
because "the same as node" is a claim two adapters can drift out of quietly.

The same harness also deploys **`examples/demo`** — 31 routes, two response-hook plugins, all five
render strategies, a streamed document, and a server action — and asks it eighteen questions that
the four-route fixture has no way to reach. The plugin defects above lived in precisely that gap.

`scripts/load-probe.mjs` measures cold start, per-route throughput and latency percentiles, and
resident memory across repeated rounds. Measured rather than gated, because every number is a
property of the machine; the one thing it fails on is a non-200, because a request that errors under
concurrency and succeeds alone is a defect whatever the hardware.

### Also

- A deploy manifest is generated and embedded in the route manifest, so an adapter reads one file.
- Marker package validation, and cycle detection reworked around the linked-bundle checks.
- `dev` server discovery improvements.

## v1.1.0 (2026-08-24)

### Breaking: an import whose case does not match the file is refused

`existsSync` and `is_file()` answer without regard to case on Windows and on macOS's default
filesystem. So `import Header from './Header'` finds `header.tsx`, the project builds, and every
check anybody runs locally is green. On Linux the same import resolves nothing at all — and Linux is
where CI runs and where the build is deployed. The failure is invisible on the machine that writes
it and arrives on the machine that ships it.

Both module graphs compare an import's spelling against the file the filesystem actually handed
back, and refuse a mismatch with `RUV1807` naming both spellings:

```
RUV1807 import "./Header" from <dir> asks for "Header", but the file on disk is named "header".
```

Scope is deliberately narrow. Only **relative** specifiers are checked: the importer's directory is
itself a resolved path, so a segment differing only in case came from the specifier and nothing
above it. Package specifiers stay out, because pnpm reaches a package through a symlink farm where
the real path differs from the request for reasons that have nothing to do with spelling. Folding is
ASCII-only, for the reason `localeCompare` is banned here — case outside ASCII is decided by the
host's locale tables, and a rule that answered differently on two machines would be the bug it
exists to prevent.

The comparison is pure string work over two paths the Rust resolver already holds, so it costs no
syscall there. The JavaScript graph needs one `realpath` and takes it only on the platforms that
fold case; on a case-sensitive filesystem there is nothing to report, because a mis-spelled import
does not resolve in the first place.

**Breaking for a project that has an import spelled in the wrong case and has only ever been built
on Windows or macOS.** That project was already broken on Linux; the build now says so while it is
still cheap to fix. Spell the import the way the file is named.

`tests/fixtures/import-case-conformance.json` holds both graphs to one answer, and its two halves
are exercised by different runners: the case-folding branch on Windows and macOS, the
unresolved-import branch on Linux. A file that exists in no casing is still an ordinary `RUV1801`,
pinned in both languages so `RUV1807` cannot take over a plain missing import.

### A CSS Module class name could name the machine that built it

`normalized_relative_path` produces the key a scoped class name hashes from, and it canonicalized
both the module path and the project root with `Path::canonicalize` — whose failure shape does not
match its success shape. Succeeding returns the Windows extended-length `\\?\` prefix; failing
returns the path unchanged. When one side succeeded and the other did not — a generated or virtual
stylesheet, whose file is not on disk — `strip_prefix` failed and the **absolute** path became the
hash input.

That bakes the build machine's directory layout into a class name shipped in the CSS and in the
JavaScript, so two machines building one project disagree about what to call it.
`pnpm verify:reproducible` cannot see it: it builds twice on one machine, where the wrong answer is
still the same wrong answer.

Both sides go through `normalized_canonical_path` now, so the two fallbacks have one shape and
`strip_prefix` succeeds in every combination. Emitted bytes are unchanged for every project whose
stylesheets are real files, which is why nothing had caught it.

`resolve_layout_file` in the route graph was handing out a verbatim path for the same reason.
Nothing downstream was wrong — `ModuleCache` normalizes on the way in — but it is the shape that
broke every server-components build once already, and the next caller has no reason to expect it.

### The two module graphs agreed on which files compile by comment alone

`MODULE_KIND_EXTENSIONS` exists twice: in `crates/ruvyxa_bundler/src/compiler.rs` for the client
graph and in `packages/ruvyxa/runtime/compiler.mjs` for the server and prerender graph. Neither can
import the other's, and the only thing asking them to agree was a doc comment on each. An extension
accepted by one and refused by the other is a build that passes on the client and fails at prerender
with `RUV1806` naming a dependency the application never wrote.

`tests/fixtures/module-kind-conformance.json` holds them now: the list and its order, the case
folding both apply, the extensions both refuse, and the extensionless package entry point both
allow.

The same pass found `CONFIG_FILE_NAMES` in `runtime/css-runner.mjs` — exported, and read by nothing
in the repository including its own module, since the commit that introduced it. The doc comment
beside the Rust list called it a mirror the two "have to agree" on or a config would be found by one
and rejected by the other. PostCSS detection happens entirely on the Rust side and the runner is
handed the resolved `configFile` path in its request, so the copy could not accept or reject
anything. A comment promising a gate that does not exist is worse than no comment, because it tells
the next reader something is watching. The copy is gone and there is one list.

`resolve_file_candidate` also canonicalized twice for every module it resolved: it probed with
`canonicalize()` and then called `normalized_canonical_path`, which already falls back to its
argument when canonicalization fails — so the branch could not change the answer. One call now, on
the hottest path in the resolver, and one fewer again in package-exports resolution, where the
existence probe and the compared path had been the same canonicalization taken twice.

### A deployed server-components page was blank in a real browser

Every check anybody had was green. The route answered `200`, the markup was right, the client
component was in it with its initial state, the Flight payload was in the document, and the module
script was there to hydrate from. Opening the page showed nothing at all, and the console said why:

```
Failed to read a RSC payload created by a development version of React on the server
while on the client using a production version of React.
```

A deployment is a production build whichever way the host starts it, and its browser half already
said so unconditionally — the Rust bundler folds `process.env.NODE_ENV` to `"production"` and cannot
be told otherwise. The server half read the _ambient_ value, and nothing in an emitted deployment
sets one. `node server/index.mjs` is the documented way to run the Node adapter's output; it exports
nothing, so the emitted server ran React's **development** build against a production browser
bundle. For an ordinary page that is a size and speed cost nobody would notice. For a
server-components route it is fatal, and fatal only in a browser: nothing observable over HTTP is
wrong.

The development build also writes an owner stack for every payload row, each frame naming an
absolute path on the machine that ran the build. The smoke's document was 11,420 bytes, of which
9,878 were stack frames publishing `D:/Ruvyxa/...` to every visitor. It is 1,542 bytes now.

Each bundle a deployment emits states the build it is, ahead of every module factory in it — the
earliest point inside a module body, and the only one that works: a statement in the _entry_ cannot
do it, because ESM evaluates a module's imports before any statement of the importer, and React
reads `NODE_ENV` while its own factory runs. Each linked bundle carries its own copy for the same
reason, since `route-modules.mjs` imports the `react-server` bundle and that sibling's body runs
first.

An `edge` artifact had the same bug arriving from the other direction and nothing could work around
it: a Worker has no `process` at all, so the stand-in compiled into each module was the only
`NODE_ENV` its React would ever read — and it was compiled from whatever the _build_ process
exported, which is nothing.

The smoke now asserts a deployed server-components document contains no `file:///`, which is the
cheap observable for both halves of this: development React's frames and the path leak are the same
string.

### A server function blanked the page it was called from, in every deployed build

Found by clicking a button on a deployed page, which is the only way it could have been found. React
posts a server-function call to `POST /__ruvyxa/rsc` — the same path that serves a route's payload
for a soft navigation. The emitted handler accepted `GET` there and refused every other verb with a
`405`, so the call failed, the promise rejected with `Connection closed.`, and React unmounted the
tree: a blank document, from a page that had rendered correctly a moment earlier.

Nothing above the browser could see it. The document was right, hydration worked, the status was
`200`, and the smoke's ten checks were green. The same page worked under `ruvyxa dev` and
`ruvyxa start`, because the native host has had the endpoint since server functions shipped.

The deployed function builds an action bundle per server-components route now — the `react-server`
build again, imported on the first call rather than at module scope, since most requests never make
one — and `POST` runs the reference the `x-ruvyxa-action` header names. Same header gate as the
`GET`, same content type on the reply, and the body bound is applied where the other endpoint bounds
are so a call accepted locally is accepted here.

The bundle is built from the union of both graphs' `'use server'` modules, which is not an
optimisation: an actions file the page imports is in the `react-server` graph and nowhere else, and
one imported only by a `'use client'` component is in the browser graph and nowhere else, because a
client reference's own imports are never walked. `examples/deploy-smoke` now has the second shape —
the one a server-graph-only bundle answers `RUV1861` for — and the smoke calls it over HTTP on Node,
Bun, and Deno.

This is the third capability that existed on one of the two request hosts and not the other, after
`/__ruvyxa/action` and the `GET` half of this endpoint.
`tests/fixtures/framework-endpoint-conformance.json` lists both verbs now, so the next one is a test
failure rather than a blank page.

### Dynamic server-components routes deploy

`RUV2213` refused every server-components route that was not pre-rendered, on every adapter, because
the generated route module rendered through the ordinary SSR entry — the page would have deployed
with no payload in the document and nothing for its browser bundle to hydrate. The adapter runner
compiles the route's `react-server` graph and its SSR registry at build time now, and the generated
module renders through `renderServerComponents`, the same pipeline `ruvyxa start` uses.
`/__ruvyxa/rsc` answers in a deployed function too, where it used to be a `501`, so a soft
navigation into such a route fetches a payload instead of reloading the document.

The remaining refusal is `RUV2202` for a target with no server, which cannot run a Flight pass at
request time whatever the runner emits.

Verified on Node, Bun, and Deno through `examples/deploy-smoke`'s `/rsc` route — deliberately
`force-dynamic`, because a pre-rendered one proves nothing about a deployment: its payload is
already inside the file the adapter copies and no renderer runs. In a browser, against the Node
artifact: the page hydrates, `useState` works, and a soft navigation from `/` into it keeps the
document alive.

Three things blocked it, each worth not rediscovering:

- **The SSR registry has to be linked _into_ `route-modules.mjs`**, not shipped beside it. It is
  compiled with React external so it shares the renderer's instance; as a sibling it resolved its
  own copy and every hook threw `Cannot read properties of null (reading 'useRef')`. The
  `react-server` bundle is the opposite case — it carries React's server build on purpose — so it
  stays a sibling.
- **Inlining a linked bundle into another one collides on `__m<N>`/`__ext<N>`.** Both number from
  zero, so the inner `const __m1` shadowed the outer one and the outer's `const __ext1 = __m1` hit a
  temporal dead zone: the deployment failed to import, with an error neither bundle could explain.
  `compileBundleWithMetadata({ identifierPrefix })` exists for this; the default keeps every other
  bundle byte-identical.
- **pnpm gave one physical React five module keys.** Each dependent gets its own symlink, and the
  graph was keyed by the path it was reached through, so a server bundle held five React instances.
  Ordinary SSR survived by luck; the RSC pass did not. The key is normalized through `realpathSync`
  now — `filePath` is left alone, because client-reference ids are measured from it. Every function
  bundle every adapter emits got 36% smaller as a side effect.

### Server functions could not be called from a deployed page

Found by opening a deployed server-components page in a browser and clicking the button on it — the
first time anybody had. Every check the repository owns was green: the route answered `200`, the
markup was right, the payload was there, the page hydrated. The click threw `Connection closed.` in
the console and left a blank document.

The native host answers both verbs on `/__ruvyxa/rsc`: `GET` renders a route's payload for a soft
navigation, `POST` runs one of the server functions that route exposes and answers with the payload
its return value encodes to. The deployed handler implemented the first and refused the second with
a `405`. So a `'use server'` function — `useActionState`, an inline `'use server'` in a server
component, an actions file a client component imports — worked under `ruvyxa dev` and `ruvyxa start`
and did not exist in production.

The build compiles the action bundle for a route now, exactly as `worker-pool.mjs` does for the
local hosts, and the generated route module loads it on the first call that needs it. Both hosts
build it from the _union_ of the two graphs' `'use server'` modules, which is not an optimisation:
an actions file the page imports is in the `react-server` graph and nowhere else, and one imported
only by a `'use client'` component is in the browser graph and nowhere else, because a reference's
own imports are never walked.

**A `<form action={fn}>` submitted without JavaScript was the same bug wearing different clothes.**
React writes the reference into hidden fields rather than into an `action` attribute, so the
submission posts to the page's own URL. `posted_form()` recognises that on the native host; nothing
in the deployed handler did, so the page re-rendered with its initial state and answered `200` —
indistinguishable from a form that was never submitted. It runs the action and replays the
`useActionState` result now, and answers `no-store` whatever the route's strategy says, because a
`ssg` route serves a file to readers and renders to submitters.

This is the third capability to have existed on one request host and not the other, after
`/__ruvyxa/action` and the payload endpoint above.
`tests/fixtures/framework-endpoint-conformance.json` is the table that is supposed to catch it, and
it did not: it probed one verb per path and said nothing about the others. It takes a list now, and
the handler is asserted not to answer `405` to anything the table lists.

### No live-rendered page in any deployed build hydrated

Found on the way to the above, and older than it. The generated route registry **is** the renderer
once the build is over, so it has to write everything a document needs. It wrote markup and nothing
else — no bootstrap block, no module preloads, no `<script type="module">` — because both writers of
those are Rust: `client_hydration_script` for a live render, `inject_prerender_client_assets` for a
baked page. Neither runs inside a deployed function.

So every SSR route in every deployed build shipped inert markup, and every ISR route lost its script
from its first revalidation onward, because revalidation persists what the registry rendered over
the file the build had injected into. Nothing logged anything.

`documentAssetsPrelude()` is the JavaScript twin of `safe_json_for_script`, `escape_html`,
`hydration_loader_url`, and the head/tail placement rules, emitted as source text because a function
bundle resolves no sibling specifiers. Per-route assets come from `<outDir>/client/manifest.json`,
the same file the Rust side reads, and are baked in as literals.

The check that matters is not "is a script present" but **"does the live render byte-match the baked
file"** — they are the same page, so comparing their tails is one line, and it is what closed this.

### `pnpm release:validate` failed on a clean working tree

With an unhandled `ENOENT` and a Node stack, naming a `package.json` nobody had touched. Four
scripts enumerated `packages/@ruvyxa/*` and opened a manifest in every entry, and a directory under
that scope is a package only if it _has_ one — pnpm resolves the same glob and skips the ones that
do not. What triggered it was ordinary: a package removed from git while its `dist/` and
`node_modules/` stayed on disk. That leaves a directory `git status` cannot even mention, because
every file in it is ignored and git has nothing to say about a directory.

One helper answers the question for all four now, the way pnpm answers it, and
`validate-package-metadata` prints a line naming any directory it skipped rather than leaving it
invisible. A package whose manifest genuinely went missing is still caught: its name stays in the
hand-maintained publish order and disappears from the discovered set, which is exactly the mismatch
`validate-release-publish-plan` exists to report.

### `ruvyxa start` was reachable on `[::1]` and refused on `127.0.0.1`

`localhost` is two addresses on any dual-stack machine, and the server took whichever one the
resolver returned first — `::1` on Windows. A browser tries the other family and never tells you;
`proxy_pass http://127.0.0.1:3000`, a container health probe, and `curl 127.0.0.1` do not, and got
connection refused from a server that was serving perfectly well:

```
curl http://127.0.0.1:3000  → connection refused
curl http://[::1]:3000      → 200
curl http://localhost:3000  → 200
```

That is the ordinary self-hosting shape, so the ordinary self-hosting setup could not reach the
server at all. The emitted adapter servers were never affected — they default to `0.0.0.0`.

A host is not an address. The server now binds **every** address the configured host answers to,
across one port, and serves them from one router with one shutdown channel. `--host 0.0.0.0` and
`--host 10.0.0.5` still bind exactly what they name; loopback is treated as one destination with two
addresses, because a client that resolves `localhost` may arrive on either.

Binding the whole set is also what checks the port is free, which removes a race: the previous code
bound one address and then probed the others, and the port-conflict logic it needed for
`ruvyxa dev`'s "two projects quietly shared port 3000" bug now falls out of holding all of them.
`--port 0` gained a fix on the way — the OS assigns per socket, so each family had been getting a
different port; the first assignment now sets the port the rest are held to.

The new test opens connections rather than inspecting listeners, since the claim is about
reachability, and asserts both families answer on one port. Reverting to a single-address bind fails
it with the reported symptom.

### A warm production build spent two thirds of its time on answers it already had

Measured on the demo, through `ruvyxa bench --baseline`, which renders each sample in a disposable
copy with a private cache and refuses to publish timings until it has confirmed the cold and warm
builds emit the same artifacts. Every scenario in the fixture moved:

| scenario          | before |  after |
| ----------------- | -----: | -----: |
| cold-build        | 7885ms | 6624ms |
| warm-build        | 1718ms |  554ms |
| first-route       |  520ms |  270ms |
| css-edit-build    | 2563ms | 1355ms |
| client-edit-build | 1925ms |  735ms |
| server-edit-build | 1819ms |  588ms |
| leaf-edit-build   | 1954ms |  870ms |

Three things were paid for repeatedly, and none of them had changed since the last build.

**The config bundle was recompiled on every boot.** Loading the plugin host compiles
`ruvyxa.config.ts` and everything it reaches into one module, then `writeIfChanged` finds the result
byte-identical to the file already on disk and writes nothing — 341ms of a 964ms build to confirm
that. The compiler could not say so any sooner, because walking the graph is how it finds out.
`compileBundleIfChanged` records what the last compile read, and what those files hash to together,
in a manifest beside the output; the next boot re-reads them instead of recompiling them.

The recorded set is every file the graph reached, not the project's own. `dependencyHash` is
deliberately project-scoped — it answers whether the _application_ changed — but 89 of the 94 inputs
to a config bundle are the framework's modules, and a key that ignored them would have served a
stale bundle to anyone editing Ruvyxa itself, silently and for as long as the file sat there.
`tests/packages/ruvyxa/bundle-reuse.test.mjs` observes reuse rather than inferring it: the emitted
bundle is marked after a compile, so a compile that really ran erases the mark. Narrowing the key
back to project files leaves five of its six cases passing and fails the sixth.

**Runtime detection spawned three processes to answer one question.** `JavaScriptRuntime::detect()`
asked Node, Bun, and Deno for `--version` — all three, because the arguments to the decision were
evaluated before the decision, so Node answering first saved nothing. A build asked six times. It is
now one probe, memoized for the process, and the test counts probes rather than checking the answer:
the previous tests pass unchanged against the eager version, which is why nothing caught this.
Running through the `ruvyxa` npm wrapper was never affected — it sets `RUVYXA_INVOKER_RUNTIME` — so
this is the standalone binary's path, and CI's.

**Pre-rendering started a Node worker pool and then had nothing for it to do.** A dynamic route's
paths come from the project's own `generateStaticParams`, so the pool has to exist before the first
job is known. On a warm build that start _was_ the phase: 158ms of 263ms, after which every render
came from the artifact cache. It cannot be skipped, but it depends on nothing the build does in
between, so it now starts next to the plugin host and the server-components collection and is
awaited where it is used. A project with no dynamic static params still starts nothing at all.

Static parameters themselves are still resolved on every build. Caching them would mean treating
`generateStaticParams` as pure with respect to its module inputs, which is a semantic decision and
not a speed one; the opt-in `{ params, cache }` return remains the way to ask for it.

### The deployment smoke was asking the demo to do something no adapter can

CI built `examples/demo` with the bun and deno adapters and then launched the result. That build
could not succeed at the time: the demo has a `force-dynamic` server-components route, every adapter
served pages through a generated module built by the ordinary SSR entry, and such a route was
refused with `RUV2213` — documented behaviour, not a gap that appeared here. `--adapter node` failed
on the demo in exactly the same way; the bun and deno jobs were simply the only ones that built the
demo with an adapter at all, which made a framework-wide limit look like a runtime-specific one.
(`RUV2213` is gone further down this release, and the demo deploys — but these jobs are about the
emitted server, not about the demo's feature list, so they still build the smaller fixture.)

`examples/deploy-smoke` is the smallest application every self-hosted adapter _can_ deploy, and it
is what those jobs build now. Node joins them, so the next time something makes an adapter refuse
the fixture, all three say so rather than two.

The smoke itself asks more than it used to. One health endpoint told you the process had started and
nothing else, and the three transports differ in how a request reaches the handler and how a file
becomes a response body — a Bun range bug that served a whole file for a sliced `BunFile` went
through a health check without a mark on it. It now checks the pre-rendered page, an ISR route, the
generated route registry, a public asset's content type and cache lifetime, the security defaults on
a rendered page, that an unknown path is a 404, both directions of content negotiation, a dynamic
server-components document, `/__ruvyxa/rsc` in both its verbs, and a real server-function call. All
eleven pass on Node, on Bun 1.4.0, and on Deno 2.9.5.

### Bun and Deno serve with their own servers

`Bun.serve` and `Deno.serve` take a function from `Request` to `Response`. `createHandler` **is** a
function from `Request` to `Response`. Between them, until now, sat `node:http`: every request had
its `Request` taken apart into a Node request, and every answer had a `Response` reassembled from a
Node one — a header list rebuilt in each direction, a body buffered on the way in, and a web stream
converted to a Node stream on the way out. Both runtimes implement `node:http` well enough that this
worked, which is why it lasted; it was never what either runtime is for.

Each adapter now emits its own runtime's server, and nothing above the transport moved. Which URL
names which file, what it is served as, how long it may be cached, which bytes a range asks for,
whether routing or the publish directory answers first, whether the security defaults apply — all of
that is one shared program text with one `staticResponsePlan` in it, and a transport turns the plan
it is handed into bytes. That is the whole reason to do it this way: **the question "does a Bun
deployment behave like a Node one" is answered by construction**, not by two implementations that
agree today. Only reading a file's bytes differs, because only that is a runtime API rather than a
decision — `Bun.file` (a slice of which is still a file, so a range is still `sendfile`), Deno's own
readable for a whole file and the Node compatibility stream for a range, `createReadStream` on Node.

The Node transport is unchanged on purpose. Rewritten as a `fetch` handler it would have paid the
same translation in the other direction, on the runtime most deployments use.

Shutdown, keep-alive, and signal handling came along. `Bun.serve`'s `idleTimeout` defaults to ten
seconds, which sits under every managed proxy's idle window and produces exactly the intermittent
502 that `keepAliveTimeout` is raised to avoid on Node; it now follows `RUVYXA_KEEP_ALIVE_TIMEOUT`
like the Node server. `Deno.serve`'s `shutdown()` already waits for in-flight responses, which is
the drain the Node transport builds by hand. And signals are registered one at a time inside a
`try`, because Deno on Windows delivers only a subset of them and throws on the rest — losing
`SIGINT` because `SIGTERM` could not be registered would leave the process unstoppable from a
terminal.

**The emitted program is now run, not read.**
`tests/packages/core/standalone-server-conformance.test.ts` stages the directory an adapter actually
produces — the real `serverless-handler.mjs` and every module it imports, copied exactly as
`materializeFunction` copies them — and puts all three servers through one table of cases: an
immutable hashed bundle, a revalidating public asset, a PNG URL answered by the WebP the build
published, a byte range checked against the exact bytes, an unsatisfiable range, a HEAD, a rendered
page, a `public/` HTML fallback, two kinds of miss, three path traversals, and the security headers
on both a static file and a page. The Node server is spawned as its own process and answers real
sockets. Bun's and Deno's are loaded with that runtime's server and file APIs standing in, which
does not claim to test either runtime — it tests the program Ruvyxa emits for them, which is the
part this repository owns. Before this, every assertion about these servers was a regular expression
over a template string, and neither Bun nor Deno was installed on the machine that ran them.

### Every production page load logged a 404 for `/favicon.ico`

A browser asks for `/favicon.ico` only when the document declares no icon of its own. `ruvyxa dev`
declared one; production did not, for two independent reasons, and both are the same kind of mistake
— a decision made in one renderer and not the other.

A pre-rendered page is composed by the build and served from disk with no renderer left to touch it,
so everything the live pipeline adds to the head has to be in the file already. The stylesheet was;
the asset links were not. And on the live path, the link is emitted when the published directory
holds the icon — but `ruvyxa dev` publishes the project's `public/`, which has the PNG, while
`ruvyxa start` publishes the staged assets, where `image.keepOriginal: false` had already converted
it to WebP. So the check asked about a file that production had by then renamed, and answered "no
icon" for every live-rendered page as well.

Both are fixed at the seam rather than at the symptom. `PrerenderHead` carries the asset links
beside the styles through the whole pre-render path, built from the staged assets directory — the
one a deployed server actually publishes, not the source directory the build read. And the icon
lookup now knows both forms the build can leave behind, declaring the type of whichever is there.
Static, ISR, CSR, streamed, dynamic, and server-components routes all carry the link now; they were
checked one at a time against a running `ruvyxa start`.

The links are in the pre-render cache key too, because they are in the output: publishing or
removing the file one names changes what a cached page would serve.

### A canonicalized project root failed every server-components build

`std::fs::canonicalize` writes an extended-length `\\?\` prefix on Windows, and a root that carries
one hands it to every module path derived from it. Node's resolver never produces one. So a build
given a canonicalized root asked its `'use server'` substitution table with `\\?\D:\app\...` while
the worker had reported `D:\app\...`, the lookup was answered by nothing, and the bundler walked the
real server module into a browser bundle — where it is refused as **`RUV1820`, naming an import the
project is right to have**. The same directory built without the prefix.

`ruvyxa bench --baseline` was the reliable way to hit it: it canonicalizes the project root before
cloning it. Every baseline run on a Windows machine with a server-components route had been failing.

The fix is one spelling for a path in the one place that compares two. `without_verbatim_prefix`
sits beside `normalized_canonical_path` and answers the other half of the question — that one asks
the file system what a path is, this one only respells a path already in hand, which is what a
lookup key needs and what a key that ran `canonicalize` per module would pay a syscall for. The
benchmark uses it too, so its workspace is the shape of path a user's build actually has.

While there: a failing baseline sample now names the scenario it failed in. Seven builds run per
sample and they differ only in what the previous step edited, so an unqualified error made a failure
after the client edit and a failure on the very first cold build read identically.

### A warm production build spent most of its time recompiling one answer

Measured on the demo, a warm `ruvyxa build` spent **1.5 of 2.2 seconds** asking the `react-server`
graph two questions about two routes: which of a route's modules are client references, and which
`'use server'` modules those references reach. Both answers come from compiling, both are a pure
function of the files that were read, and neither depends on the request — so both were being
recomputed from scratch on every build of an unchanged project.

They are cached now, content-addressed on those files exactly like every other build artifact, and a
build where every route hits starts no worker process at all. The demo's warm build went from ~2.15s
to ~1.47s.

**What makes it safe is the input list.** The `react-server` graph reads a `'use client'` module —
it has to, to see the directive — and then stops, so the `'use server'` module _behind_ one is known
only to the registry compile. The reference ids in the cached answer are versioned by that module's
source, so an answer kept across an edit to it would hand the browser a proxy naming a function the
server no longer registers, and every call through it would fail at run time. The worker reports the
union of both compiles' inputs for that reason, and
`tests/packages/ruvyxa/server-component-entry-inputs.test.mjs` holds it there.

The same gap was open in the render path, where it reached further: a pre-rendered server-components
page is cached against these inputs too, and the registry is what supplies the client components
React renders into the HTML — including the hidden fields of a `<form action={fn}>`, whose reference
id is versioned by the action module's source. Editing that module left every pre-rendered page
serving markup naming a function id the server no longer registered. That render reports the union
now as well.

The collection also overlaps the plugin host's start, since neither needs anything from the other
and both are dominated by a JavaScript runtime coming up.

### The slowest thing `ruvyxa dev` served was its own route table

`/__ruvyxa/client/route-manifest.json` is what the client router matches against, and the browser
fetches it on every document load. Serving it meant asking the worker pool for **every** page
route's browser bundle — the hash of that bundle is the `artifactVersion` the router compares
against — reading each route's source, and parsing it, all of it again per request. Measured over a
raw keep-alive socket on the demo: a page took **0.2 ms** and this took **60 ms**, and the first
request of a session took **3.3 seconds** because it compiled twenty-seven browser bundles end to
end before answering.

It is cached now, and the routes are compiled across the pool rather than one after another: **0.10
ms** warm, **0.87 s** on the first request. What it costs after an edit — the only other time it is
built — is 120–170 ms.

**The cache is dropped on every watcher event, including the selective ones.** That branch exists to
keep the route manifest and the collected CSS across an edit that changes neither, and it was the
one that mattered here: a component edit changes the bundle that component is in, which is exactly
what this table advertises. Kept across one, it would tell the router that the copy the browser
already holds is current, and the next soft navigation would render the code from before the save.
`a_selective_watcher_event_still_drops_the_client_route_table` holds it, and was seen fail without
the line.

### Two JavaScript runtimes started one after the other

`ruvyxa dev` came up in **1.09–1.17 s**, of which 0.76 s was the plugin host starting and 0.21 s was
the render worker pool starting. Neither needs anything from the other. They come up together now,
and the middleware stack is validated before either is spawned rather than after — a rejected
configuration is a configuration error, and paying for two runtimes to boot before reporting it only
makes the report slower.

**0.62–0.68 s.** The first request to a server-components route came down with it, from 1.34 s to
0.55 s, because the pool now has a head start on its dependency warm-up.

### Bun and Deno, measured rather than read about

Both runtimes were installed on the machine this was written on — Bun 1.4.0 and Deno 2.9.5, under
`~/.bun/bin` and `~/.deno/bin` — and `ruvyxa doctor` reported Deno as missing. Both installers add
their directory to `PATH`, and a shell that has not been restarted since sees neither the executable
nor a shim; the lookup now falls back to where each installer writes, so `--runtime deno` can be
selected at all.

With that fixed, `tests/packages/core/standalone-server-conformance.test.ts` runs all three emitted
servers **as real processes** when the runtime is installed, and falls back to stubbed
`globalThis.Bun` / `globalThis.Deno` when it is not. The suite title says which happened, because
the two are not the same evidence — and the difference showed immediately.

**`BunFile.slice(…).stream()` serves the whole file.** Measured on Bun 1.4.0: a sliced `BunFile`
reports the right `size`, and `text()`, `bytes()`, and using it directly as a response body all give
the four bytes asked for — but the same slice's `.stream()`, serialized by `Bun.serve`, sends all
ten. A `Range: bytes=2-5` request came back `206` with a correct `content-range` and the entire file
as its body, which is the whole video a seek would have played. The slice is handed over as a file.
Bun does **not** re-apply a request's `Range` to a response the handler returns — its automatic
`206` is about its own static `routes` — so nothing has to work around that either.

`Bun.serve`'s `idleTimeout` was also being set to 65 seconds on the theory that Bun closes an idle
connection after ten. It has defaulted to 0 — never — since Bun 1.1.27, which is already on the safe
side of the 502 the Node transport raises `keepAliveTimeout` to avoid, and is what lets a long
streamed response stay open. It is left alone now unless `RUVYXA_KEEP_ALIVE_TIMEOUT` asks for a
bound.

`ruvyxa doctor` reports each runtime against the version the emitted server needs — Bun 1.1.26, for
`Bun.serve`'s `idleTimeout`, and Deno 2.0, for the Node built-ins it imports — and flags one below
it. A `--version` line it cannot parse is reported as-is rather than as too old: a cosmetic change
in someone else's output must not become a toolchain this tool declares broken.

`ruvyxa build --root examples/demo --runtime bun` and `--runtime deno` both build the demo in full,
server components included, and `ruvyxa dev --runtime bun` / `--runtime deno` both come up and serve
every route class — a page, a client-hydrated page, a server-components page, and an API route.

### PostCSS ran under Node whatever the project selected

A project's PostCSS chain is the project's own JavaScript, loaded from the project's own
dependencies, and every other JavaScript stage of a build already runs under the runtime the project
selected. This one launched `node` unconditionally — so a Bun- or Deno-only machine built everything
except its stylesheet. The runtime travels to `PostcssRunner` now, and
`the_plugin_chain_runs_under_the_projects_runtime` reads the program and the arguments the runner
would launch.

### React Server Components

A page can opt in with `export const serverComponents = true`. It and its layouts then run in a
module graph resolved with React's `react-server` condition, and only the modules marked
`'use client'` reach the browser.

```tsx
// app/dashboard/page.tsx
import { readFile } from 'node:fs/promises'
import Chart from './chart' // 'use client'

export const serverComponents = true

export default async function Dashboard() {
  return <Chart rows={JSON.parse(await readFile('./data/metrics.json', 'utf8'))} />
}
```

`page.tsx` is never bundled for the browser. `chart.tsx` is, and on that route it is the only module
that is. The page becomes a _payload_ — a serialised element tree in which `Chart` is a reference id
rather than code — which the server renders to HTML and the browser replays to hydrate. The payload
rides in a `<script type="application/json">` data block, so a `Content-Security-Policy` without
`'unsafe-inline'` does not block it.

`react-server-dom-webpack` is an optional peer, installed only by apps that use the export; a route
that opts in without it gets `RUV1863` naming the package.

**Both React instances live in one process.** Ruvyxa compiles the server graph itself, with
`react-server` in the resolver's condition list, so React's server build is linked into that bundle
and the ordinary React stays outside it. A worker thread started with `--conditions=react-server`
was measured to work too, and was rejected: it cannot run on the worker runtimes the adapters
target, and bundling React is something this codebase's linker already does for every client bundle
it emits. Everything here was established by running `react-server-dom-webpack@19.2.8`, not by
reading its documentation — including that `server.browser` reads `__webpack_require__.u` at module
load and throws in a plain Node process, and that the client side asks for exactly two globals,
which are a chunk loader and a registry.

**One authority per question.** The `react-server` graph is the only thing that knows which of a
route's modules are client references, so it also writes the browser entry — but the _Rust_ bundler
compiles it, because that is where `NODE_ENV` folding, tree-shaking, minification, and the chunk
budget live. Building it in the JavaScript compiler instead was tried and produced a 1.5 MB bundle
carrying both of React's builds for a page with one button on it. `bundle_entry_source` is the new
seam.

**Combinations that would silently do nothing are refused at discovery** (`RUV1011`): a
`'use client'` page has no server half; partial pre-rendering streams a shell through an entry this
pipeline does not build; an intercepting route is matched from a client route registry a
server-components browser entry does not publish. A route that still needs a server at request time
is refused for adapter builds (`RUV2213`), because every adapter serves pages through a module built
by the ordinary SSR entry — a pre-rendered one deploys anywhere, since its payload is already in the
HTML file the adapter copies.

Also in the tree, each with tests: `BundleTarget::ReactServer` and a matching `react-server` target
in `runtime/package-exports.mjs`, pinned by five cases in `module-resolution-conformance.json`
including `react-server-dom-webpack`'s own exports map; `runtime/client-references.mjs`, which owns
the id a `'use client'` module carries across the three graphs that must agree about it; and
`runtime/rsc-client-runtime.mjs`, the browser half, which has no imports at all because the client
bundle inlines it.

### Server-components documents stream

A server-components route whose document is produced per request no longer waits for the whole tree
before its first byte leaves. React sends the shell as soon as it has it and each `Suspense`
boundary as the server resolves it. Measured on `examples/demo/app/streaming`, whose two boundaries
wait 300ms and 1200ms:

```text
buffered   first byte 1.79s   complete 1.79s
streamed   first byte 0.62s   complete 1.81s
```

**Only a per-request document streams**, which is the same dividing line as everything else about
caching: a pre-rendered, `revalidate`, or statically rendered route has to become a _string_ to be
written to disk or held in a cache, and a stream is not one. A route without server components does
not stream either — its render resolves in one step, so it would trade a working document cache for
nothing. `streams_document()` is where that is decided, and a test spells out every strategy.

A streamed response is `Cache-Control: no-store`, has no `Content-Length`, and is not stored in the
render cache. That is inherent rather than an omission, and it is why the rule above is narrow.

**The host still composes the document, on a stream instead of a string.** `StreamedDocument` makes
the same two edits [`compose_localized_document`](crates/ruvyxa_dev_server/src/html_document.rs)
makes: the asset links and stylesheet go in once `</head>` has been seen, and the hydration script
goes in before `</body>`, found in a rolling window of the last kilobyte. Neither the head bound nor
the tail window can hang the response — a document with no head is forwarded unedited, and a
document with no closing tag gets its tail appended.

**The Flight payload rides in the frame that ends the body.** It is complete only when the render
is, by which time the first frame is long gone, so `emitApiStream` grew a `streamTrailer` and the
body stream fills a slot the composer reads at the end. The payload therefore stays exactly where it
was — one `<script type="application/json">` data block near `</body>` — because the browser needs
it to _hydrate_, not to paint, and hydration cannot begin before the document has been parsed.

The doctype is emitted only when React did not emit one itself, which is the difference between a
tree that starts at `<html>` and one that does not. Prepending unconditionally produced two.

One thing worth knowing when testing this: React defers revealing a completed boundary to a
`requestAnimationFrame`, and a hidden document never runs one. A streamed page in a background tab
shows its fallbacks until the tab is looked at. That is React's scheduling, not this framework's.

### React server functions

`'use server'` is implemented, in both of the shapes React defines. A whole module behind the
directive, which is what a `'use client'` component imports:

```ts
// app/dashboard/actions.ts
'use server'
export async function rename(id: string, name: string) {
  await db.rename(id, name)
  return db.get(id)
}
```

and one function inside the server component that uses it, handed down as a prop:

```tsx
export async function markAllRead(userId: string) {
  'use server'
  await db.markAllRead(userId)
}
```

Either way the browser gets a _reference_, never the code. Calling it posts the arguments to
`POST /__ruvyxa/rsc` — the same endpoint that already serves a route's payload, because it is the
same question asked twice — runs the real function in the `react-server` realm, and resolves to what
it returned. The reply is a Flight payload rather than JSON, so a server function may return an
element tree containing client components.

**A `'use server'` module is the mirror image of a `'use client'` one, and the machinery is
deliberately the same shape.** The server graph compiles it and registers its exports; every client
graph replaces it with a proxy of `createServerReference` calls. The id is computed by the one rule
in `runtime/client-references.mjs` that already answers for client references, with an `s_` prefix
so the two can share a registry and a `__webpack_require__` without ever being confused.

Registration is enqueued and flushed rather than run in place: Ruvyxa's linker emits a module's
`__exports.name = name` assignments _after_ its body, so code at the bottom of that body sees an
exports object that is still empty. The generated entry flushes before it renders anything.

**An inline server function must be at the top level of its module** (`RUV1867`). One declared
inside another function closes over that call's variables, and making it callable from a later
request means hoisting it and binding what it captured — which needs a scope-resolving parser this
graph does not have. Compiling it anyway would produce a function that reads values from a render
that ended, so it is refused with the file and line instead. Anonymous default exports, object
methods, and callbacks are refused for the same reason: the registration has to name a binding that
is still in scope where it is emitted.

**Three things the work uncovered, each a real defect on its own:**

_`ruvyxa build` could not compile a browser bundle that reached one._ The Rust bundler walks the
real file, and a module behind the directive — or merely _named_ `actions.ts`, which is the action
lane by filename in both graphs — is `RUV1820` inside a client bundle. That answer is right for
every route with no way to call a server function and wrong for a server-components route. The
worker now reports the stand-in source per file and a build hook serves it in place of what is on
disk, so there is still one implementation of what a reference looks like; and the proxy declares
`'use client'`, which is not a trick to get past the lane rules but a true statement both of them
read ahead of the filename.

_Only the server graph measured reference ids from a stable base._ `ruvyxa build` compiles a route
from the project's sources and `ruvyxa start` compiles it from the copy staged under
`<out>/server/`, two directories deeper, so an id measured from the project root differs between
them. The server graph already used the directory holding the app directory; the browser and SSR
graphs did not, because until now they computed no ids of their own. In development the two bases
coincide, so this surfaced only under `ruvyxa start`, as `RUV1865` for a function the process was
holding all along.

_`compiler.mjs` never consulted its `external` option._ It held by accident: a server bundle leaves
`node_modules` to Node's resolver anyway. The SSR client registry then needed both `external` and
`bundlePackages` — it inlines client modules lifted out of their packages, so it must carry their
dependencies — and resolved its own React despite listing it. Client components rendered against
that second copy read a null dispatcher and threw on the first hook.

### A form action works without JavaScript

`<form action={fn}>` no longer waits for the page's bundle, and no longer needs it at all. React
writes the function's reference into hidden fields while rendering the form; a submission from a
browser that has run none of the bundle posts those fields to the page's own URL, and Ruvyxa runs
the function before rendering the response. The result is in the document that comes back.

`useActionState` is what puts it there. React writes an extra key beside the hook and
`decodeFormState` turns the return value into the token the HTML renderer replays it from, so one
component renders one answer whether it was reached over `fetch` or over a form post. A form using
anything else still runs its action; it just has nowhere to show the return value, which is what
that spelling means.

**A submitted form is answered by the route's own render, not by its strategy.** Whatever a
pre-rendered or cached document holds was produced before this action ran, so the render is fresh
and the response is `Cache-Control: no-store` — and a `Ssg` route with a form is now a route that
serves a file to readers and renders to submitters. Anything the action passed to `revalidatePath()`
is applied before the response is returned, so a visitor following a link on the answering page
cannot beat the invalidation to the cache.

The request carrying a form is also the one page render that is **not retried**. The pool replaces a
worker that dies mid-flight and re-sends anything idempotent; a render that runs a server function
first is not, and `is_idempotent` now matches on the absent body rather than on the variant.

**Recognising one is three questions, none of which is "does this name an action".** `POST`, a form
content type, and a server-components route — all answerable from the request line and one header.
Whether the body actually names a reference needs a multipart parser and the function registry, both
of which live in the worker, and a post that turns out to name nothing renders the page exactly as a
post always did there. Guessing wide costs a body forwarded; guessing narrow leaves a form silently
inert for every visitor without JavaScript.

`decodeAction` resolves the reference through React's own `__webpack_require__` rather than through
Ruvyxa's resolver — a hidden field is decoded by React, so the id takes React's route — which is why
the action entry now installs the reference runtime before decoding. The `'use server'` module
itself is unchanged: the same function answers a click handler and a form post, because
`useActionState` calls an action with the previous state and a `FormData`, which is also what a
browser submits.

### The shared browser chunk sorted its modules by pathname

A production build lifts every module used by more than one route into one shared chunk, and route
bundles `import` it — so the whole chunk is evaluated before a route's own first statement. The list
of modules in it was collected into a `BTreeSet`, which sorted them by path. That is a real
reordering of the browser's work, and it is invisible until two modules have a load-order dependency
the module graph cannot express.

One does. `react-server-dom-webpack/client.browser` reads `__webpack_require__.u` while its own
module body runs, and the module that defines that global is `rsc-client-install.mjs` — with no
import between them, because the vendor package has never heard of Ruvyxa. The route entry imports
the install module first and a test asserts that it does, but sorted by path
`node_modules/react-server-dom-webpack/…` comes before `packages/ruvyxa/runtime/…`, and the entry's
order stopped mattering the moment both landed in the shared chunk. **Every server-components page
threw `__webpack_require__ is not defined` and never hydrated in production** — no `useState`, no
server function, no form. `ruvyxa dev` builds one bundle per route and was unaffected, which is why
it survived a full round of browser verification.

The shared chunk now evaluates modules in the order the routes evaluate them.
`PreparedBundle::module_paths()` returns a `Vec` — it always computed the order, through
`ordered_project_modules`, and then threw it away on the last line — and `shared_route_module_paths`
walks the routes in build order, taking each module the first time it is seen. Each route's list is
already dependency-first, so nothing lands ahead of what it depends on.

Both registry emitters take the order they are given: `bundle_shared_prepared_route_modules` seeds
its walk from it, and `bundle_shared_route_modules` writes its synthetic entry in it. They have to
agree — a build where every route was prepared in-process takes the first and a build with a cached
plan or a build hook takes the second — and the test that pins them together now compares order as
well as bytes.

Two cache formats moved with it. The client plan is version 3, because a version 2 entry holds the
same modules sorted by path and that is a different answer; and the shared-chunk artifact key
includes the order, because the order is in the output. A cold build and a warm build of the demo
produce the same shared chunk filename.

### Server-components routes are navigable

`router.push('/dashboard')` into a route that opted into server components now renders it in place.
The router fetches the route's Flight payload from the new `/__ruvyxa/rsc` endpoint and hands it to
a factory the route's browser bundle registered, instead of the tree factory an ordinary route
publishes — a server-components route has no page in the browser for a factory to build a tree from.
Navigating back out works the same way it always did.

The endpoint is deliberately not the same contract as `/__ruvyxa/flight`. That one serves Ruvyxa's
own public, cacheable route payload and refuses a request carrying credentials; this one is a
_render_, so it forwards the visitor's headers — a server component may read `cookies()` exactly as
it does on a full request — and its response is `private, no-store`. A same-origin header keeps it
out of reach of a cross-origin page, which cannot set one without a preflight nothing answers.

A deployed function answers it with 501: it serves pages through modules built by the ordinary SSR
entry, which is why `adapter-runner.mjs` already refuses a server-components route that still needs
a server (RUV2213). On those targets the navigation falls back to a document load, which for a
pre-rendered route is a static file the CDN already holds.

### Every soft navigation in `ruvyxa dev` was a full page load

Client-side routing worked in a build and never once worked in development. Nothing reported it,
because a document load renders the same page: the address bar changed, the content changed, and
only the round trip and the lost client state gave it away. Two independent causes, each sufficient
on its own.

**The route manifest claimed a Flight export on pages that had none.** `has_named_runtime_export`
strips the export keyword and a declaration prefix off the source, then asks whether the remainder
starts with the name it is looking for — and answered "yes" when the strip _failed_. Any
`export const meta = …` therefore matched every query, so the dev manifest advertised `flight: true`
for pages with no `flight(context)` anywhere in them, the router prefetched a payload the route
cannot produce, the request came back 501, and the navigation fell back. The same predicate gates
`RUV1842` at build time, which consequently never fired.

**A development bundle never said which build it was.** The bundler appends
`__RUVYXA_ROUTE_ARTIFACTS__[route] = version` to every browser bundle it emits, and the router reads
it back after importing a route to prove the bundle matches the version the manifest named. The dev
server compiles browser bundles in the Node worker, which never wrote it, so the check found nothing
and treated every freshly imported bundle as stale. `/__ruvyxa/client` stamps at serve time now,
with the version hashed from the unstamped script — the route manifest and the Flight endpoint hash
the same text, so all three agree without any of them having to tell the others.

### A server component could not use the framework's own components

A root layout that renders a `<Link>` nav is the ordinary case, and it took down the entire
server-components render with `Class extends value undefined is not a constructor or null`: nothing
in `@ruvyxa/react` declared a client boundary, so the `react-server` graph compiled `Link`,
`RuvyxaErrorBoundary`, the routing hooks, `Script`, and `useRuvyxaLoader` for real — against a build
of React that has no `Component` to extend, no `createContext`, and no effects. Those five modules
carry `'use client'` now and become references; `Image`, `Seo`, `notFound()`, and the barrel itself
stay server-side, so a server component still renders them itself.
`packages/@ruvyxa/react/test/client-boundary.test.mjs` asserts the split against `dist`, because the
directive only does anything if it survives `tsc`.

Marking them surfaced two more, both in the graph rather than the package:

**A dependency's client reference had a different id in each tree.** The id was the module's path
relative to the directory holding the app directory — stable for a project file, which is what the
earlier fix measured, and not stable at all for a package: `ruvyxa build` reaches it from the
project and `ruvyxa start` reaches it from the tree staged under `.ruvyxa/server`, two directories
deeper. A direct load of such a page worked and every soft navigation into it failed with `RUV1861`.
A module inside a package is named by the package now — everything up to the last `node_modules/` is
dropped and one is put back, which also makes pnpm's store layout and npm's flat one agree.

**`external` was never consulted.** The option held only by accident: a server bundle leaves
`node_modules` to Node's resolver anyway, so nobody noticed the list was unread. The SSR client
registry then needed both — it inlines client modules lifted _out_ of their packages, so it must
carry their dependencies or a bare specifier resolves from `.ruvyxa/cache/rsc/` and comes back
"Cannot find package" — and with `bundlePackages` on it resolved its own React despite listing it.
Client components rendered against that second copy read a null dispatcher and threw on the first
hook. A listed external is now answered before resolution, exactly where a rewritten one already
was.

### A development browser bundle hydrated with the wrong build of React

Browsers have no `process`, so every module the JavaScript compiler wraps gets a stand-in — and it
said `NODE_ENV: 'production'` unconditionally. `ruvyxa dev` deliberately leaves `NODE_ENV` unset and
therefore _renders_ with development React, so the two halves of every development page disagreed
about which build they were. Ordinary hydration tolerated it. A Flight payload does not: the
server-components route failed with React refusing to read a development payload on a production
client. The stand-in mirrors the compiling process now, which is the invariant that matters — both
halves of one render come from one worker.

It also means a development bundle finally reports React's real error text instead of a minified
error code.

### Three bugs the work above uncovered

**No `ruvyxa dev` client bundle could hydrate.** `react-dom/client` came out of the JavaScript
compiler with a bare `import "scheduler"` in it, which no browser can resolve, so the module never
evaluated and hydration never started. The cause is pnpm's layout: `node_modules/react-dom` is a
link into a store directory whose siblings are react-dom's own dependencies, and the resolver walked
the link path — where `scheduler` was never installed — instead of the real one. Node resolves from
a module's real path, and the Rust resolver did too because everything reaching it has already been
through `normalized_canonical_path`. This graph now does the same.

**A side-effect import ran last, whatever the source said.** The specifier scan uses one regex per
import _form_ and appends each pattern's matches, so the bare `import './polyfill.js'` form — the
last pattern — always sorted after every `… from …` import. Matches are ordered by where their
specifier appears now. The first pattern can also start at one `import` keyword and run past a
side-effect import to reach the next `from`, so the position used is the capture group's, not the
match's.

**`ruvyxa start` served on-demand development bundles to the client router.**
`/__ruvyxa/client/route-manifest.json` was synthesized from the live route table in every mode, and
its entries pointed at `/__ruvyxa/client?path=…` — a bundle compiled per request, carrying its own
React. Soft navigation therefore rendered a component from one React copy into a root owned by
another, every hook in it threw, and the router fell back to a document load. Pages still worked,
which is why nothing reported it. A build's `route-manifest.json` is served verbatim when one
exists.

### A client reference had two ids

`ruvyxa build` compiles a route from the project's sources; `ruvyxa start` compiles the same route
from the copy the build stages under `<out>/server/`. Measured from the project root those give
`app/counter.tsx` and `.ruvyxa/server/app/counter.tsx` — two ids for one module — so the payload a
running server rendered named a reference the browser bundle had never registered, and the first
navigation into the route rendered nothing. Ids are measured from the directory holding the app
directory, which is the position both trees share.

### Server renders were using React's development build

Nothing set `NODE_ENV` for a rendering worker, so `ruvyxa build` and `ruvyxa start` both loaded the
development build of React: slower, noisier, and — for a server-components route — one that writes
absolute source paths into the payload the browser receives. Both now default it to `production`,
and only when the project has not set it itself. `ruvyxa dev` is unchanged and still gets the
development build, which is the one it wants.

Pre-rendering the demo went from 4.96s to 963ms on the same machine.

### Two import scanners read comments as imports

`scripts/pack-smoke.mjs` and `tests/packages/ruvyxa/worker-runtime-contract.test.mjs` each walk
`runtime/*.mjs` for relative imports, so a new sibling module cannot work in the monorepo and then
be missing from the published tarball. Both matched the text with a regex and neither masked
comments or strings — so a doc comment containing an example `import … from './counter'` sent both
looking for `packages/ruvyxa/runtime/counter`, and both failed on a file that was never meant to
exist.

They mask through `createCodeIndex` from `runtime/scanner.mjs` now — the same tool the rest of the
repository already uses for exactly this. Two copies of one scan with one hole is what "count the
copies before trusting a check" means in practice: fixing the comment would have left the second
scanner waiting for the next one.

### Intercepting routes are implemented

`(.)`, `(..)`, `(..)(..)`, and `(...)` are real now, rather than refused. A folder carrying one is
an **overlay** on a route that already exists: a soft navigation to the URL it names renders it into
a parallel-route slot while the page underneath stays mounted, and a hard load of the same URL
renders the ordinary page.

```text
app/gallery/
├── @modal/
│   ├── (.)photo/page.tsx   ← over /gallery when the router navigates to /gallery/photo
│   └── default.tsx
├── layout.tsx              ← receives `modal` alongside `children`
├── page.tsx
└── photo/page.tsx          ← what a reload or a shared link renders
```

The target is computed from the **level's** URL, never from the slot folder, because a slot
contributes no URL segment: for `app/gallery/@modal/(.)photo`, `(.)` names the level `app/gallery`
and the target is `/gallery/photo`. Route groups contribute none either. Getting that wrong is
invisible until a modal silently never opens, so it is pinned by
`tests/fixtures/intercepting-route-conformance.json`, which both discovery implementations replay —
`route_intercepts()` in `crates/ruvyxa_graph` for `ruvyxa build`, and `collectIntercepts()` in the
new `packages/ruvyxa/runtime/route-intercepts.mjs` for `ruvyxa dev`.

**The interception is carried by the route you are standing on, not by the route it covers.** That
is what lets the overlay open with no request at all — its component is already in the running
bundle — and it is the same fact that makes a reload show the real page: nothing else publishes a
table for that URL. The intercepted route's own entry is untouched.

Two things fail the build rather than doing nothing:

- **RUV1006** — a marker whose target no page answers, or one that climbs above the app root. An
  overlay with no real route behind it is a modal that never opens and a URL that 404s.
- **RUV1005** — a marker outside an `@name` slot. There is nothing to render it into. (This code was
  introduced earlier in this release to refuse every marker; it now refuses only the ones no slot
  can show.)

A slot that can be intercepted is emitted as `__ruvyxaSlot(ctx, level, name, Default, table)` rather
than a fixed element, because the overlay is decided per render and a slot may hold nothing but an
interception. Both entry generators emit it identically — two new cases in
`tests/fixtures/entry-composition-conformance.json` pin that — and a route with no interceptions
emits neither the resolver nor the table, so its bundle is byte-identical to before.

The tree is rendered with the **mounted** page's pathname while an overlay is open. `template.tsx`
is keyed on it, so handing the tree the overlay's URL would remount every template on the chain —
and with them the page the overlay exists to sit on top of. The overlay component receives the
intercepted URL and its parameters as its own props, and the router snapshot follows the address bar
so `usePathname()` outside the tree and `useSearchParams()` agree with what the user sees.
`examples/demo/app/gallery` exercises the whole path.

### One rule decides which file a bare import names

Ruvyxa resolves every import twice: `crates/ruvyxa_bundler/src/resolver.rs` walks the graph for
`ruvyxa build`, and `packages/ruvyxa/runtime/compiler.mjs` walks it again for the dev server, the
prerender workers, and every function artifact an adapter assembles. The two disagreed, and not at
an edge — at the centre.

`compiler.mjs` answered bare specifiers with `createRequire(...).resolve(specifier)`. That is Node's
**CommonJS** resolver: it matches the conditions `["node", "require"]` and nothing else. Against a
package exporting
`{ "require": "./cjs.js", "browser": "./browser.js", "worker": "./worker.js", "import": "./esm.js" }`
it picked `cjs.js` — for a browser bundle, for an edge Worker, for everything. The Rust bundler,
resolving the same import for the same build, picked `browser.js`, `worker.js`, or `esm.js`
according to the bundle target. Neither side raised anything. The build reported one bundle and the
other graph shipped a different one.

The `exports` decision now lives in one place, `packages/ruvyxa/runtime/package-exports.mjs`, and
`tests/fixtures/module-resolution-conformance.json` holds both languages to it: condition order per
target, the two-pass treatment of `require`, subpath patterns and their longest-prefix tie-break,
explicit `null` blocking, the legacy `browser`/`module`/`main` order, and which package-relative
paths may be joined at all. The fixture stores each `exports` field as source _text_ rather than as
parsed JSON, because condition order is what the rule reads and a map that sorts its keys would lose
it.

Writing the table found a defect neither implementation knew it had. An `exports` map with no
`.`-prefixed key is sugar for `{ ".": <that map> }` — it defines the root entry and nothing else —
and both hosts were answering _subpaths_ from it. `import x from 'pkg/sub'` therefore resolved to
the package's **root** file: not an error, just the wrong module. Both now leave it unmatched, which
falls through to the legacy branch and probes `pkg/sub` itself. Node refuses the subpath outright
here; falling through is the documented divergence, taking the wrong file was not.

`compileBundleWithMetadata` gained a `bundleTarget` option (`client` / `ssr` / `edge`) because
`platform` was answering two questions at once. An edge artifact is compiled with
`platform: 'browser'` — a Worker has no Node resolver at runtime — but it must read `worker` and
`edge-light`, not `browser`. `adapter-runner.mjs` states its target; everything else takes the
default derived from `platform`, so no existing caller changes behaviour except by being right.

### Benchmarks re-measured against this tree

`README.md`'s comparison table was re-run on 2026-08-21 with
[`scripts/bench-frameworks.mjs`](scripts/bench-frameworks.mjs) against Next.js 16.3.2 and Astro
7.2.4, using a development build of this branch packed from source rather than a published release —
the column is labelled `1.0.32-dev` for that reason. The previous table was measured on 2026-08-05
against 1.0.28, a different Node/npm/pnpm set, and different Next/Astro versions, so the two runs
are not comparable to each other; only the three columns _within_ each run are.

`ruvyxa bench --baseline` also still passes `tests/fixtures/build-bench-contract.json` on the demo
after the transform and minifier changes below, which is the check that a language-level pass added
to both compilers did not quietly cost build time.

### Intercepting-route folders are refused instead of published

`(.)`, `(..)`, `(..)(..)`, and `(...)` are Next.js conventions Ruvyxa does not implement. Nothing
stripped them either: the route-group branch needs a trailing `)`, so `app/feed/(.)photo/page.tsx`
went straight through segment validation and mounted a real, publicly reachable page at
`/feed/(.)photo`. A view written to be shown over another route got its own public address, and no
diagnostic said so. Inside a `@slot` the same folder matched no URL and rendered nothing.

Route discovery now fails with **RUV1005** for any folder under `app/` whose name opens with one of
the four markers. The scan walks directories rather than the segments of discovered routes, because
the route walk skips `@slot` folders and a marker inside one is just as wrong; `_`-prefixed folders
are excluded, since they opt out of routing entirely and nothing in them can reach a URL. When more
than one folder qualifies the reported one is chosen by sorted path, so two machines building the
same project name the same file.

A convention this framework does not implement has to fail loudly or work. This is the same rule
that made `export const dynamic` and `generateStaticParams` honoured rather than silently ignored.

### One answer to "who is this request from"

Every per-client control in the framework keys on a client identity: the built-in `rate` middleware,
the server-action rate limiter, and the action replay guard's per-client quota. Two implementations
answered that question and they disagreed.

`RateLimitLayer::extract_key` read the transport peer and never looked at a forwarded header, while
`clientAddress` in `packages/ruvyxa/runtime/serverless-handler.mjs` scanned `X-Forwarded-For` from
the right against `security.trustedProxyIps`. One project with one `middleware.builtin.rate` block
was therefore limited per real client once deployed, and as **a single shared bucket** when the
native server ran behind a reverse proxy — where every caller arrives with the proxy's address. The
control meant to protect the service became the thing that denied it: one client exhausting the
bucket answered 429 to everyone.

The rule is now one module, `crates/ruvyxa_middleware/src/client_ip.rs`, which both Rust hosts call
and which `tests/fixtures/client-ip-conformance.json` holds against the JavaScript host.
`security.trustedProxyIps` reaches the middleware stack through
`MiddlewareStack::with_trusted_proxies`, so `key: "ip"` means the transport peer unless that peer is
loopback or a listed proxy, in which case the forwarded chain names the client. A client that is not
a proxy still cannot rename itself.

What stays outside the shared table is deliberate and mirrors `ForwardedScheme` in
`@ruvyxa/core/origin-policy`: whether this request's upstream hop may be believed at all. The native
server weighs the transport peer; a deployed function has no peer and treats its platform ingress as
trusted by construction. Everything after that decision is identical.

`key: "header:<name>"` is unchanged and still verbatim — an API key is not an address and must not
be parsed as one — but pointing it at `x-forwarded-for`, `x-real-ip`, or a platform ingress header
hands the bucket key to the caller, who can rotate it for an unlimited allowance. The server now
warns at startup when it sees that, and names `ip` as the proxy-aware mode instead.

`docs/{en,th}/13-security.md` also lost two claims that were no longer true: built-in CSRF-shaped
protection exists as the `originGuard` plugin, and general rate limiting exists as
`middleware.builtin.rate`. Both are opt-in, which is worth saying — but "no codebase evidence
establishes" them was wrong.

### `build.esTarget` compiles to the target it names

`build.target` in `ruvyxa.config.ts` was accepted, validated, carried all the way into
`BundleOptions` — and consumed by neither compiler. The Rust bundler built
`TransformOptions::default()` with no `env`, and `runtime/compiler.mjs` hardcoded `target: 'esnext'`
in its `transformSync` call. A project that set `es2018` got byte-identical esnext output and found
out in a browser.

Both compilers apply it now, from one spelling: `EsTarget` in `crates/ruvyxa_bundler/src/types.rs`
reaches `TransformOptions::from_target` on the Rust side and the `target` option on the JavaScript
side, carried to the worker in `RUVYXA_ES_TARGET` the way the JSX runtime already was. It is back in
the compile cache key and in the prerender context hash, both of which it was deliberately left out
of while it selected no transform. `tests/fixtures/es-target-conformance.json` holds the two graphs
to one accepted list, because a value one accepts and the other refuses is a build that succeeds on
the client and fails at prerender.

**Downlevelling is not free of runtime support, and that is the reason this was inert for so long.**
oxc's helper loader defaults to `Runtime`, so a transform that needs a helper emits
`import _x from "@oxc-project/runtime/helpers/x"`. That package is in neither module graph, oxc's
`Inline` mode is `unreachable!()`, and a deployed function bundle resolves no bare specifiers at all
— the import would reach production as a module nothing can find.

Which targets need a helper is a property of the source, not of the number: ordinary application
code compiles helper-free at es2022 and above, a private class field pulls helpers in from es2021
down, and a single `using` declaration needs one at every target below es2026. So the refusal is on
the **emitted** code. Both compilers scan what they produced for a helper import and fail by name if
one appears, rather than accepting the configuration and shipping something unresolvable. The scan
reads imports rather than matching output text, so a module whose source merely quotes the specifier
is not flagged.

`es5` is not an accepted value: oxc does not implement it, and
`every_accepted_es_target_is_one_oxc_accepts` checks the advertised list against the transformer
rather than against a comment promising the two agree.

The minifier had to learn the target too, and this is the half that only an end-to-end check finds.
oxc's compressor rewrites toward the shortest equivalent form, and the shortest form is frequently
newer syntax: it turns `a.b ?? (a.b = 0)` — which is exactly what the transform had just produced —
back into `a.b ??= 0`. With `CompressOptions::target` left at its default, a project on
`build.target: es2020` had a correct unminified build and shipped logical assignment in the minified
one. The compressor is now held to the same level, and the profile the build already selected is
unchanged: only `target` is set on it, so asking for a language level does not quietly change how
much is compressed away.

### Navigation paints the destination immediately

A soft navigation used to hold the previous page on screen until the target route's Flight payload
arrived. On a slow route that reads as a dead click: the user pressed a link and nothing changed.

Every client entry for a route that has a `loading.tsx` now also emits a `__ruvyxaShell` factory —
the route's layouts wrapped around its loading component — registered under
`globalThis.__RUVYXA_SHELLS__`. The router paints it as soon as the route's bundle is available and
replaces it with the real tree when the payload lands.

The shell needs **no server round-trip**. Its layouts and loading component are already inside the
bundle that `<Link prefetch>` warms, so there is nothing left to fetch by the time it is painted;
this costs no request the navigation was not already making. The URL is committed at the same
moment, so the address bar cannot lag behind what the user can see, and a payload that then fails
falls back to a full load without pushing a second history entry.

A route with no `loading.tsx` has declared no loading state and keeps the previous behaviour — the
old page stays up. Inventing a blank screen for it would be worse than the page the user is already
looking at.

Both entry generators emit it, `crates/ruvyxa_bundler/src/output.rs` and
`packages/ruvyxa/runtime/entry-templates.mjs`, so a project renders the same way under `ruvyxa dev`
and in a built bundle. Server entries carry no shell: a server render has its data in hand and never
shows the fallback.

Splitting the bundle wait from the payload wait pushed `navigate` past the `complexity` gate. It was
split along the seams it already had — `needsRouteLoad`, `commitShell`, `settleFlight` — rather than
by hoisting fragments out to move the number.

### `error.tsx` can retry against the server

The fallback receives `{ error, reset, retry }` where it previously received `{ error, reset }`.

The two recover from different failures and are deliberately not merged. `reset` clears the boundary
and re-renders against data the client already has, which recovers from a fault in the render
itself. `retry` aborts any request still in flight for the route, discards the cached payload,
re-fetches it, and only then clears the boundary — which is what is needed when the failure _was_
the data. One button that silently did the wrong one would look like it worked.

A failed retry replaces the error rather than clearing it, so the boundary never shows children
whose data still is not there. On a page with no mounted router there is nothing to re-fetch from,
so it degrades to a `reset` instead of doing nothing. `router.retry()` backs it, and the exported
`RuvyxaErrorBoundary` gained the same pair so a hand-written boundary is not weaker than the
convention one.

### `params()` reads route parameters from any depth

A page already receives its parameters as props. `params()`, exported from `ruvyxa/server`, is for
everything below it — the shared formatters, loaders, and nested components that need a `[lang]`
segment and used to reach it by prop-drilling, or by re-parsing the URL themselves.

Unlike `cookies()`, `headers()`, and `draftMode()`, **reading it does not make the render
request-dependent**. A parameter is part of the route's identity rather than of who is asking:
`/th/blog/hello` renders the same document for every visitor. Recording a request-state read here
would have quietly dropped any page that read its own parameters out of static rendering and out of
the ISR cache — the opposite of what the API is for.

It resolves in API route handlers too. A server action is invoked at its own endpoint rather than
matched against a route pattern, so it has no route parameters, and `params()` reports that rather
than returning an empty object — otherwise a mistyped segment name would read as "this route has no
such parameter".

### Reproducible builds, measured rather than asserted

Ruvyxa already enforced the ingredients of a reproducible build — `localeCompare` banned, ordering
through explicit comparators, the Rust and JavaScript graphs held to shared conformance fixtures.
Nothing checked the property those rules exist to produce.

`pnpm verify:reproducible` builds a project twice from clean and compares every emitted file. It
sorts the differences by what they mean rather than reporting one list: emitted code differing is a
defect and fails; build telemetry (`build.json`'s `createdAtUnix` and `timing`, and the cache
counters in `client/manifest.json` that `ruvyxa bench` reads) describes how the build _ran_ and is
reported without failing; a prerendered page differing is almost always the application's own clock
or random value, which Ruvyxa cannot tell apart from a bug. `--strict` fails on all three, which is
what attesting that an artifact matches a commit needs.

The demo builds to 91 emitted files that are byte-identical across two clean builds.

### Deployed servers survive a deploy

The standalone server that the node, bun, deno, aws, railway, and render adapters emit was correct
about paths, cache headers, and body limits, and had nothing at all about process lifecycle.

- **Graceful shutdown.** Every container platform stops a deploy with `SIGTERM` and a kill shortly
  after. Node's default is to exit immediately, dropping every response still being written — so a
  rolling deploy showed users connection resets. The server now stops accepting, drains in-flight
  responses, and closes idle keep-alive sockets so the drain is not held open by connections
  carrying nothing. A request that never finishes cannot outlive the platform's own grace period.
- **Keep-alive above the proxy's idle window.** Node closes an idle connection after 5 seconds while
  AWS ALB idles at 60. When the proxy believes a pooled socket is good and the origin has already
  begun closing it, the request that lands on it fails — a 502 that appears only under load, only in
  production, and only intermittently. `keepAliveTimeout` now defaults to 65s with `headersTimeout`
  above it; both are overridable by environment variable.
- **A bad route cannot take the process down.** An unhandled rejection terminates Node by default,
  taking every concurrent request with it, so it is now reported and the server keeps serving. An
  uncaught exception is treated differently on purpose: the process state is no longer trustworthy,
  so it drains and exits non-zero for the supervisor to replace.
- **Stream failures are contained.** Both the static-file pipe and the response-body pipe commit
  their status before the body flows, so a later failure can only end the connection — and left
  unhandled, that `error` event was fatal to the whole process. A client that disconnects now also
  stops the file read, and stops a render still producing for nobody.

The generated server is assembled as a template string, so `tsc` validates the template and never
the program that comes out of it — the first execution is on a user's host after a deploy. A test
now writes the emitted source out and parses it, alongside assertions for each behaviour above. The
same guard was added for the route boundary emitted by `entry-templates.mjs`, after an unescaped
backtick inside a comment closed that template early and produced a class that could not compile.

### An ISR cache write no longer fails the page it rendered

`writePrerendered` failures escaped the request on the foreground paths. A runtime filesystem that
is read-only is ordinary in production — a container started with `--read-only`, a pod with
`readOnlyRootFilesystem`, Cloud Run, a Lambda bundle outside `/tmp` — and so is a full disk. Storing
the render is a cache optimisation; serving it is the request. A page that had already rendered
correctly returned 500 to every visitor because the write that came after it threw.

Ordinary ISR and SSG writes are now best-effort and degrade to rendering on each request. A write
that settles a `revalidatePath()` claim still surfaces: the caller was promised that the next
request sees the new document, and this instance cannot reach instances already warm elsewhere, so
the durable write is the only thing that makes the promise true. Swallowing it would report success
while every later request kept serving the old page; the claim stays pending for retry instead.

### Host locale can no longer change build output

`localeCompare` was banned for deciding ordering by the host's ICU locale. Two case-folding calls
were doing the same thing and were not covered.

`compiler.mjs` generated heading slugs with `toLocaleLowerCase()` while the native compiler's
`slugify` lowercases with Rust's locale-independent `char::to_lowercase` — so on a Turkish host `I`
became `ı` in one and `i` in the other, and the two compilers disagreed about a heading `id`.
`contentTitleFromRoute` used `toLocaleUpperCase()` for a title baked into generated metadata and
prerendered HTML, so the same project built to different bytes on different machines.

Both now fold locale-independently, and `no-restricted-properties` covers `toLocaleLowerCase` and
`toLocaleUpperCase` alongside `localeCompare`. A unit test cannot catch this class — CI runs on
en-US, where the assertion passes whether or not the bug is present — so the lint rule is the gate
that fires on every host. The `searchIndex` plugin still folds by locale, because the locale there
comes from the project's own configuration rather than from the host, and carries a documented
exemption on each line.

### Auth keys are owned by the runtime that owns the secret

Derived HMAC keys lived in a module-global `Map` keyed on the secret. It had no eviction, so every
distinct secret the process ever saw kept both its plaintext — as the map key — and its derived key
alive until the process exited, and discarding an auth runtime could release neither.

Each runtime now holds its own lazily-imported key, which ties the key's lifetime to the object that
owns the secret. Signature bytes and the never-cache-a-rejected-import behaviour are unchanged; a
session issued by one runtime is still readable by the next one started from the same secret, which
is what makes a redeploy or a second worker safe, and is now asserted directly.

The development server's process-global client-manifest cache had the same shape and is now bounded
to 128 roots with oldest-first eviction. Eviction costs a re-parse and never a wrong answer.

### React's streaming scripts can be covered by a policy now

Converting the route bootstrap to a data block left one inline script: a route that streams Suspense
content carries React's own runtime, the script that swaps a resolved boundary into place. It is
React's, not Ruvyxa's, so it cannot be moved out of an executable element — and the artifact holding
it is written once and reused by every request, so a per-request nonce would be baked in and
therefore public.

Its bytes are fixed once that artifact is written, which is what makes a hash the right mechanism.
They are per-document, though: React's swap script names the boundary ids it completes, so
`$RC("B:0","S:0")` differs from page to page and no one can maintain the list by hand.

`securityHeaders({ inlineScriptHashes: true })` has the build record them. `build.onComplete` walks
the prerendered documents, hashes each executable inline script, and writes `csp-inline-hashes.json`
into the output directory; each response then picks up the hashes for the document it is serving. A
route with no inline script gets its policy unchanged, and a missing manifest — a development
server, or a deployment that did not enable this — sends the policy without the extra sources rather
than failing the response.

`script-src` has to be in the policy already. A policy that deliberately falls back to `default-src`
is left alone: narrowing it to exactly these hashes would block the application's own bundles, which
is a different decision than the one this option was asked to make. Data blocks and `src` scripts
are deliberately not hashed — the first is never executed and the second is already governed by
`script-src`.

In `examples/demo` this is one route out of twenty: `/ppr-page` gains two sources, and every other
page's policy is untouched.

### The bootstrap is data, not script

Every rendered page carried an inline `<script>` that assigned `globalThis.__RUVYXA_ROUTE_PARAMS__`
and `__RUVYXA_REQUEST_PATH__`. Any `Content-Security-Policy` without `'unsafe-inline'` blocked it,
and hydration never started — so a project could not adopt a strict policy at all. A CSP hash could
not cover it either, because the parameters differ per request.

It is now a `<script type="application/json">` data block. The browser does not execute a data
block, so `script-src` does not apply to it, and a strict policy needs no nonce for it. The
generated client prelude reads the block and assigns the same globals, which is why `router.ts` and
every other reader is unchanged.

Four writers emitted that script — `client_hydration_script`, the CSR shell in `render_pipeline.rs`,
and **two** in `prerender.rs`. The fourth wrote only the path and the CSR flag, so a search for the
route-params global did not find it and it kept shipping an inline script after the other three were
converted; it turned up only by grepping the demo's built output.
`tests/fixtures/client-bootstrap-conformance.json` now holds the element id and key names, both
preludes are executed against it rather than pattern-matched, and the Rust mirror is compared
byte-for-byte with the JavaScript one.

A route that streams Suspense content still emits React's own inline runtime, which no hash can
cover. Those routes need `'unsafe-inline'`, or a policy that does not cover them.

`headScriptHashes(plugins)` is exported from `ruvyxa/plugins` for the other case: a plugin's `head`
entries are identical on every request, so they are covered by a `'sha256-…'` source rather than a
nonce.

### Route parameters could close the CSR shell's script element

The CSR shell interpolated `serde_json::to_string` output straight into an inline `<script>`.
`serde_json` escapes `"` and `\` but leaves `<` alone, and these parameters come from the request
URL, so a path segment containing `</script>` closed the element and ran whatever followed it. The
dev server's other writer and the prerender writer already went through `safe_json_for_script`; this
one never did.

Every writer now escapes before building the block, and
`bootstrap_block_cannot_be_closed_by_a_route_parameter` replays the escaping cases from the shared
fixture — asserting first that each case is genuinely dangerous unescaped, so the test cannot pass
by testing nothing.

### One cross-site rule instead of three

Whether a request is provably same-origin was decided by three separate implementations: the action
endpoint, the native server, and — once it was written — the `originGuard` plugin. They were kept in
step by a comment saying they mirrored each other, which is the arrangement that let
`STATIC_CONTENT_TYPES` and `DEFAULT_SECURITY_HEADERS` drift apart in production.

`@ruvyxa/core/origin-policy` is now the single JavaScript implementation, copied into
`runtime/origin-policy.mjs` for function bundles the same way `route-match.mjs` is. The native host
keeps its own copy — it is a different language — and both replay
`tests/fixtures/origin-policy-conformance.json`.

The one input the three legitimately disagree about is the trusted scheme, so the table takes it as
an argument rather than deriving it: the native host supplies one only when the transport peer is in
`security.trustedProxyIps`, a deployed function reads `X-Forwarded-Proto` as stated because its
platform ingress is the trusted proxy by construction, and the plugin has no trust policy and
supplies none. Splitting the decision at that seam is what made a shared table possible.

`scripts/sync-route-match.mjs` became `scripts/sync-shared-runtime.mjs` and takes a table of
modules; `pnpm --filter ruvyxa sync:route-match` is now `sync:runtime`. `origin-policy.mjs` is
registered in the package's `files`, in `HANDLER_RUNTIME_FILES`, and in `WORKER_RUNTIME_FILES` — the
last because extracting code out of `action-runtime.mjs` would otherwise have taken it out of the
prerender cache's identity, letting the rule change while every hash stayed equal.

### CORS answered the same request differently in development and production

**Breaking for a project that configured `middleware.builtin.cors` without naming `methods`.**

The native server filled an unset `methods` with an implicit `GET, POST, PUT, DELETE, OPTIONS`. The
serverless handler had no such default. A project that wrote

```ts
middleware: {
  builtin: {
    cors: {
      origins: ['https://app.example']
    }
  }
}
```

therefore answered a cross-origin `PUT` preflight with a method allowance under `ruvyxa dev`,
`start`, and `preview`, and with no `Access-Control-Allow-Methods` at all once deployed to a
serverless adapter — which the browser reads as a blocked request. Nothing failed a build or
appeared in a log; the split only showed up in a browser pointed at production.

`methods` now defaults to empty in both hosts, and an empty list sends no header. The narrower
behaviour won because `docs/en/13-security.md` already asks a project to name its origins, methods,
and headers explicitly, and closing the gap the other way would have widened the cross-origin
surface of every deployed application that had relied on a default it never wrote down. **Name the
methods you serve cross-origin**; `ruvyxa dev` now shows what production will do.

Both hosts replay `tests/fixtures/cors-conformance.json`, which also pins where each negotiation
header belongs, `Vary: Origin` on a rejected origin, and the refusal to grant credentialed access
through a wildcard — the native server rejecting that configuration before it binds a port, the
handler treating no origin as allowed because a deployed function has no startup to fail in.

### Shared tables the serverless handler was never held to

`STATIC_ASSET_EXTENSIONS` and `DEFAULT_SECURITY_HEADERS` each had a fixture, replayed by the Rust
host and by `@ruvyxa/core` — and a third copy in `packages/ruvyxa/runtime/serverless-handler.mjs`
that nothing replayed. That copy is the one running in every deployed serverless build, so the two
gated implementations agreeing proved nothing about the deployed one.
`tests/packages/ruvyxa/serverless-shared-tables.test.mjs` holds it to both fixtures now, and each
fixture's own note says three implementations rather than two.

### Parallel routes

A `@name` folder beside a `layout.tsx` declares a slot that layout receives as a prop, alongside the
page it already renders as `children` — the same convention Next.js uses, for the same reason: a
dashboard whose panels are separate files rather than one page that renders everything.

```tsx
// app/dashboard/layout.tsx — app/dashboard/@team/ and @activity/ become props
export default function Layout({ children, team, activity }) {
  return (
    <div>
      {team}
      {activity}
      {children}
    </div>
  )
}
```

Slots match the URL independently of the page: at `/dashboard/reports`, the page comes from
`reports/page.tsx` and the team panel from `@team/reports/page.tsx`. A slot with nothing for the
current URL renders its `default.tsx`, and a slot with neither is left out entirely — the layout
does not receive the prop, rather than receiving an empty wrapper the author never wrote.

Before this, a `@name` directory was pruned from the route walk and produced nothing at all: no
route, no slot, and no diagnostic. A project that wrote one got silence.

Slot props are ordered by slot name, so the emitted source does not depend on the order a filesystem
listed the directories in — the same reason every other ordering in this repository is explicit.

Both module graphs discover and compose them, and
`tests/fixtures/entry-composition-conformance.json` pins the emitted source. What is **not**
covered: a slot's own nested `layout.tsx`/`loading.tsx` is not composed into the slot subtree, and
an unmatched slot falls back to `default.tsx` on every navigation rather than retaining what it last
rendered — Next.js keeps the previous slot state on a soft navigation, which is a client-router
behaviour rather than a composition one.

### `template.tsx`

A `template.tsx` beside a `layout.tsx` wraps that level's children the way the layout does, and
differs in the one respect that is the whole reason the file exists: it is given a key derived from
the request path, so navigating within the same layout remounts it — state resets and effects run
again — while the layout above it stays mounted. Same convention, same semantics, and the same
nesting as Next.js: `layout > template > children`, at every level.

The two interleave by directory rather than being flattened. Putting every template inside every
layout is the tempting shortcut and it is wrong: a layout below a template would end up outside it,
which is observable the moment a template provides context. A level may have either file, both, or
neither.

Both module graphs emit it — the Rust bundler for `build`/`preview`, the Node entry templates for
`dev` and the SSG path — and `tests/fixtures/entry-composition-conformance.json` now pins the
composed source for the template shapes alongside the layout-only ones it already held. A route with
no `template.tsx` emits exactly the loop it always did, so nothing about an ordinary route's bundle
changes because the feature exists.

The loading shell takes the same wrappers as the tree. It is painted during a navigation and then
replaced by the tree, so a shell that wrapped the loading state differently would change the element
structure under the user mid-navigation.

### Source maps describe the file that was shipped

`build.map: true` produced a map that did not correspond to the bundle beside it, in any
configuration.

`emit_prepared_bundle` told the source-map builder where each module began using three constants
describing someone else's output: how many lines `output::wrap` prepends, how many the linker's
header takes, and how many its per-module preamble adds. All three were wrong. The wrapper prepends
nothing for a client bundle and one line for a server one, not two or three; the linker's header
sits below a variable-length block of hoisted external imports; and the preamble is longer than the
constant said. Measured on a two-module project, the first module's first statement was mapped to a
blank line twelve lines too early, and the error compounded with every module after it. Tree-shaking
and minification then rewrote the text those mappings described, and neither was accounted for at
all. Finally the CLI prepended `import "./shared.<hash>.js";` to every route that reads the shared
registry — after the map was built — shifting the whole file one line further.

Nothing noticed, because the builder's own tests fed it offsets and read back what they fed, and the
bundler's map tests read `sources` and `x_google_ignoreList`.

Positions are now recorded while the text is produced, at every stage:

- The linker returns the provenance of every line it emits, counted rather than assumed — a rewrite
  is not always one line in and one line out.
- Tree-shaking carries that provenance forward. A pass never deletes a line, so the output is a
  line-for-line descendant of its input.
- Minification hands back oxc's own positions, which is the only thing that can survive a pass that
  rewrites the text wholesale.
- The output wrapper's prefix is measured with `strip_suffix`, which proves the wrapper only
  prepends instead of assuming it.
- `sourcemap::shift_generated_lines` moves a finished map when a caller prepends to the bundle.

Resolution is per line: the linker rewrites a module one line at a time, so a line is the finest
position it can honestly report. `every_mapped_token_lands_on_the_line_that_produced_it` runs the
whole pipeline in all four minify/tree-shake combinations and resolves each token the way a debugger
does — the mapping with the greatest generated column at or before it — then requires the source
line it names to be the line that token came from.

Collapsing the sequential and parallel link paths onto one writer was part of the fix rather than a
tidy-up: the module wrapper was written out twice, byte for byte, and the source map reads its
shape.

### Next.js route conventions no longer fail silently

Two conventions a page brought over from Next.js relies on were read by nothing, and neither said
so.

`export const dynamic` is the route segment config that overrides the automatic strategy.
`force-dynamic` on an otherwise-static page was discarded and the page was pre-rendered anyway — the
opposite of what it asked for, with no diagnostic. It is honoured now: `force-dynamic` takes the
route off the pre-render path, `force-static` and `error` put it on, and `auto` is the default it
already was. It is read before the `revalidate` and `ppr` opt-ins, which is the precedence Next
defines — a page carrying both `force-dynamic` and `revalidate` is dynamic.

`generateStaticParams` is Next.js's name for the static parameter set, with the same contract:
return the parameter objects to pre-render. It is accepted alongside `getStaticParams` and
`staticParams`. Both halves that decide this had to learn it together — the route graph decides
whether a page _has_ a parameter set and the worker decides what to call when it does, and a name
recognised by one and not the other is a route that discovers as SSG and then pre-renders nothing.
`tests/packages/ruvyxa/static-params-names.test.mjs` holds the two lists to each other.

`export const metadata` is deliberately **not** aliased to `meta`. Next's metadata object is nested
(`openGraph`, `twitter`, `alternates`) where Ruvyxa's is flat, so accepting the name would
half-work, which is worse than not accepting it.

### An aliased import made a page static that a relative one kept dynamic

`detect_render_strategy` pre-renders a route with no dynamic segments when nothing in its reachable
graph reads request-dependent data. The walk that produces that graph followed relative specifiers
only, and dropped everything else without a word — so `import { getPosts } from '@/lib/posts'`
produced no edge at all, and a page whose data fetching lived one alias away looked exactly like a
page that fetched nothing. It was rendered once at build time and never again. Written
`../../lib/posts`, the same file and the same `fetch` kept the route dynamic.

The walk reads the bundler's `TsConfigPaths` now, so it resolves a project's aliases the way the
bundler that compiles the page does. A bare package specifier is still outside it, and is asserted
that way rather than left to chance: following `node_modules` would find `fetch(` in almost any
dependency and take automatic pre-rendering away from every page.

### A link prefetch was read as a data fetch

The data markers that decide whether a route may be pre-rendered were matched with `contains`, and
`prefetch(` contains `fetch(`. `prefetch` is an API this framework ships on `useRouter()`, so a page
that warmed one link was classified as a page that fetched data and lost its static rendering — a
collision with the ordinary case rather than a contrived one.

A marker has to be its own identifier now. Only the leading edge is checked, because every marker
already ends at a `(` or a `.`, and a member access has to keep counting: `globalThis.fetch(` is a
fetch. A byte that begins a multi-byte character reads as a word boundary, so a non-ASCII identifier
makes the marker count — the safe direction, since a false marker costs a static page while a missed
one ships stale data.

### Artifact keys are paired by module, not by position

`prepare_bundle_with_parts` published one Resolve artifact per module in the resolved graph, then
walked the compiled modules alongside them with `zip`. Every artifact key after that point depended
on the compile pass returning the same modules in the same order — an invariant nothing stated and
nothing checked, whose failure mode is silent: a module takes another module's Transform key, and
the next incremental build reuses the wrong artifact. `zip` would also have dropped the tail of a
longer list without a word. The keys are looked up by module path now, and a compiled module with no
counterpart in the graph is an error instead of a mispairing.

### Two framework routes a plugin could have crashed the server with

`RESERVED_FRAMEWORK_ROUTES` exists so a plugin declaring a realtime or presence transport on a
framework path gets RUV1701 instead of what axum does about it, which is to panic with
`Overlapping method route` while the router is being built — the server dies at startup, before it
can report anything. Its doc comment said it must stay in sync with the router chain, and that
comment was the only thing holding the two together. It named eight of the ten routes the chain
registers: `/__ruvyxa/hydration-loader.js` and `/__ruvyxa/client/route-manifest.json` were
registered and not listed, so a transport on either passed the guard and reached the panic.

Both are in the list now, in all three copies — the Rust array, `RESERVED_FRAMEWORK_PATHS` in
`plugin-http.mjs`, and `tests/fixtures/framework-endpoint-conformance.json`.

The two checks that existed both read the contract outwards: contract to the Rust array, and
contract to the route chain. Neither read the chain inwards, which is the direction the guard
depends on. `every_registered_route_is_reserved` reads the `.route("…")` literals out of
`build_app_router` and requires each one to be reserved, so a route added without a matching entry
fails here rather than in a user's terminal.

### A stylesheet saved mid-collection was served a save behind

`RuntimeCache::styles` reads its generation, drops the lock, collects off-thread, and installs the
result only if the generation still matches. That is what makes a watcher event during a collection
safe: the event bumps the generation and the stale result is refused.

`invalidate_styles_for_paths` skipped the bump whenever the slot held no cached value, on the
grounds that there was nothing to invalidate. But an in-flight collection is exactly the state where
the slot holds none, and the file set it compares against does not exist until a collection has
finished — so the one moment it needed to invalidate is the one moment it could not decide, and it
answered no. The collection then installed CSS it had read before the save, and the dev server kept
serving the previous stylesheet until the _next_ CSS change: the shape of "my edit did not show up,
so I saved again and then it did".

An empty slot is bumped now. The component-only optimisation this method exists for is untouched —
that path has a cached value, which is what makes the question answerable.

### The generated sitemap is parsed, not matched

`site_discovery.rs` builds XML with string concatenation, and every test asserted on the text it
produced — the generator and the assertion agreeing because they were written together. A search
engine does not read the text. It parses the document, and a sitemap it cannot parse is rejected
whole, silently, long after the build reported success.

The emitted documents are checked against the rules a parser enforces now: escaping, nesting,
namespace prefixes, and the XML 1.0 character range, over a corpus carrying every character
`escape_xml` handles in every kind of field that reaches the document — element text, attribute
values, and the `loc` built from a route path. Shards and the sitemap index are included, and
`robots.txt` is held to its own line grammar.

Namespace prefixes are the rule worth naming: `sitemap_header` declares `xhtml`, `image`, and
`video` only when `sitemap_features` saw a reason to, and `sitemap_entry_xml` emits those prefixes
from the same three fields. Nothing held those three functions together, and a prefix used without a
declaration does not parse at all. Dropping the `xhtml` declaration now fails.

Neither ecosystem here carries an XML parser and a dependency added for one test is a dependency the
release has to keep, so the checks are written out — and held by
`assert_the_checker_rejects_what_a_parser_would`, because a checker that accepted everything would
have made all of this green.

### Static files answer byte-range requests

`public/` is where a project puts its video and audio, and the server had a streaming path added
specifically for files that large — but nothing on either host spoke `Range`. A media element does
not download a file and play it; it asks for the bytes it needs as it needs them. Every seek
restarted the download from byte zero, and a strict player refuses a resource whose server will not
answer its opening `Range: bytes=0-1` with a `206` at all. The streaming path could not play the
files it existed for.

Both hosts answer ranges now. A single range returns `206` with `Content-Range` and only the bytes
asked for; a syntactically valid range past the end returns `416` with the real length; a
multi-range request is answered with the whole file, which RFC 9110 permits and which is cheaper for
everyone than a `multipart/byteranges` body no client of this server needs. `If-Range` is honoured,
so a client resuming against a file that has since changed is handed the current one whole rather
than a splice of two versions. Above the streaming threshold the reader seeks and is bounded, so a
seek to the end of a large video reads its bytes rather than the whole file.

`ruvyxa start` and a standalone/node deployment serve the same `public/` directory, so the rule is a
shared table — `tests/fixtures/byte-range-conformance.json` — answered by `parse_single_byte_range`
in Rust and `parseByteRange` in `serverless-handler.mjs`, the latter imported by the generated
standalone server rather than copied into it.

Writing the table down immediately caught a disagreement neither side would have noticed alone: the
two languages had permissive number parsers that were permissive about different things. Rust's
`u64::from_str` accepts a leading `+`; JavaScript's `Number()` accepts `1e1` and `0x2`. Both now
take ASCII digits and nothing else, and a position too large to represent stays a position — it is
past the end of any real file, so it is a `416`, not a reason to send the whole file.

### A retired worker was nobody's to close

Retiring a worker takes it out of `NodeWorkerPool::workers` and leaves a detached task to drain and
close it. `NodeWorkerPool::shutdown` only ever walked `workers`, so from the moment of retirement
the `node` child belonged to no one but that task — and both retirement paths are ordinary traffic,
not edge cases. `ruvyxa build` retires a worker every `RUVYXA_PRERENDER_RECYCLE_AFTER` isolated
renders, and `recycle` retires the whole generation whenever instrumentation changes, so creating
these is among the last things a build does before it exits. A process that exits does not unwind
that task, nothing drops the `Child`, and `kill_on_drop` never runs: the worker was orphaned, still
holding its handles on the build directory.

The pool now keeps a retiring worker until its drain finishes, and `shutdown` closes those too.
Closing one also releases its pending set, which is what the drain task is waiting on — so shutting
down unblocks the drain rather than racing it.

`shutdown` also closes workers concurrently. Each one waits up to `WORKER_SHUTDOWN_TIMEOUT` for its
process to exit, and one at a time that is two seconds per worker between Ctrl-C and the terminal
coming back, for waits that have nothing to do with each other.

### A stream frame the body rejects releases its pending entry

Ending a stream and releasing the worker's pending entry are different things, and
`WorkerBodyStream` was treating one as the other. The stdout reader only removes an entry it has
seen a terminal frame for, and `api-start` is not terminal — so a worker that repeats it mid-stream
ended the body while leaving the entry behind, with nothing left to remove it. `in_flight` never
returned to zero after that: `select_worker` permanently read the worker as the busiest in the pool,
and retiring it sat out the full `WORKER_DRAIN_TIMEOUT` before the process was closed.

### The render cache is exercised under contention

The index and the recency order live behind two locks, not one. Every method that needs both takes
them index-first, and that ordering is the only thing between this cache and a deadlocked dev server
— but every test in the file drove it from a single task, so nothing could have noticed if it broke.
Inverting the two acquisitions in `put` now hangs
`the_index_and_the_order_agree_under_concurrent_access` and leaves the other thirty-six green.

The assertions are structural on purpose. Which writer wins a race is not the cache's promise; that
the two halves still describe the same set afterwards, that capacity still holds, and that the cache
still answers, are. A second test does the same for `revalidatePath()` claims, the one piece of this
cache whose size an application controls.

### Serving a prerendered document is held to the shared path table

`settle_prerendered_artifact` had a test refusing to write outside the prerender directory. The
reader — the half an unauthenticated request reaches, which turns a URL into a file path and returns
the bytes — had none. It now replays `tests/fixtures/prerender-path-conformance.json`, the same
table the static-asset handler and the deployed handler answer to, so a path the native server
refuses is refused everywhere.

Both of the reader's defences are pinned, which took two different cases. Deleting the path rule is
caught by the conformance table. Deleting `contained_public_asset` is caught by nothing a
written-out traversal can reach — `..`, `\`, and `:` are already refused one layer up — so the test
also points a symlink out of the directory, which is exactly the shape the canonical containment
check exists for. On Windows, where creating one needs a privilege an ordinary session lacks, that
case is skipped rather than failed.

### The byte scanner is checked against the real parser

`crates/ruvyxa_bundler/src/ast.rs` is the crate's only byte scanner. Every masked-code decision in
the linker, the minifier, the boundary check, and the route graph rests on it, so a miss there is a
miss everywhere — and it has taken repeat regressions, each one a case where text was read as code.

It now answers to oxc. Forty adversarial sources — a specifier quoted inside a template, a regex
holding a comment opener, a division that follows a closing paren, JSX text with braces in it, a
string ending in an escaped backslash, a generic arrow that reads like JSX — go through both the
scanner and the real parser, and the static edges must match exactly. The one deliberate difference
is named: a type-only import is erased at compile time and is not a dependency.

Masking is checked as a separate property, because every caller reads an offset out of masked code
and then slices the original at it: same length, same line breaks, every byte either kept or
blanked, and no quoted specifier left readable.

Sources whose regex holds something that matters are in the corpus on purpose. `skip_string` stops
at a newline by design, so a mis-scanned regex cannot swallow the rest of a file — which means a
case whose regex holds nothing consequential proves nothing about regex detection. The one that
bites holds a whole `import` statement: read as division, it invents a dependency on a package the
project never installed.

`compiler.rs` gained the matching pair. Decorator stripping deletes bytes, so thirteen sources whose
`@` is _not_ a decorator — a `@media` block inside a styled-components template, `@supports`,
`@keyframes`, a JSDoc tag, an email address in a comment, `@scope/pkg` in a string, `@support` in
JSX text, a regex holding `@` — must come through with every fragment intact. And the whole `.js`
path is walked end to end: expand, link, parse. The two predicates gating expansion and the linker's
one-statement-per-line requirement are three separate pieces of knowledge about one rule, and they
had already drifted once.

### A `NODE_ENV` guard with an `else if` broke the production bundle

**Fixed.** `fold_production_node_env` removes development-only branches while a production client
graph is being resolved — it is what keeps React from pulling both its development and production
builds into one browser bundle. It took the consequent's closing brace as the end of the statement
whatever followed, so

```js
if (process.env.NODE_ENV !== 'production') {
  warn()
} else if (flag) {
  run()
}
```

lost its `if` and left a bare `else if` behind. Both directions failed: a `!== 'production'` guard
stranded the `else`, and a `=== 'production'` guard left the rest of the chain dangling after the
branch it kept. The result was a bundle that does not parse, produced by a pass that runs during
resolution and reports nothing.

The fold now walks the whole `if`/`else` chain, so the branch that survives knows where the
statement it replaces actually ends. A clause it cannot measure — a brace-less `else` — leaves the
guard untouched rather than half-removed: the bundle still defines `process.env.NODE_ENV`, so an
unfolded guard is correct, just larger. `else` is matched as a keyword now too, so an identifier
like `elsewhere` is not mistaken for one.

### A CommonJS build could beat the ESM build beside it

Node matches `exports` conditions in the order the package author wrote them, and the first
supported one wins. `require` sat in the same list as `import`, so

```json
{ "exports": { ".": { "require": "./cjs.js", "import": "./esm.mjs" } } }
```

handed `cjs.js` to a browser bundle that had `esm.mjs` sitting right beside it. Ruvyxa emits ESM, so
`require` is now a second pass: a package that ships nothing else still resolves through it, and one
that ships both gets the build that matches the output format. Author order still decides between
conditions that legitimately compete — `browser` before `import` picks `browser`.

`package_exports_resolution_matches_the_documented_rules` in `crates/ruvyxa_bundler/src/resolver.rs`
pins sixteen shapes, including the two places this resolver **deliberately** differs from Node: an
unlisted subpath falls through to the legacy fields instead of raising
`ERR_PACKAGE_PATH_NOT_EXPORTED`, and only an explicit `null` blocks. Both are now arguments a change
has to have with a test.

### Four ordinary export shapes failed the build

**Fixed.** Both linkers rewrite ESM a line at a time, and each decided what a line was by matching
text rather than by asking the scanner. Four shapes a project writes without thinking about any of
this failed:

| Source                                                          | Before                                                                                                                                |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `export const note = 'copied from here'`                        | `RUV1612`, build fails                                                                                                                |
| `const conditions = { import: './index.mjs' }`                  | `RUV1612`, build fails                                                                                                                |
| `export function* gen() {}` / `export async function* gen() {}` | `RUV1612` in the Rust bundler, `RUV1700 Unexpected token 'export'` in the Node graph                                                  |
| A Prettier-wrapped `export {\n  a,\n  b,\n}` in a `.js` module  | Bundle emitted with a stray `a,` and `}` loose inside the module IIFE — **no build error at all**, and a browser that cannot parse it |

Each had its own cause, and the last is the one that mattered most because nothing reported it:

- **`from` is ordinary English.** The re-export branch was chosen by `line.contains(" from ")` over
  the raw line, and every path out of that branch that is not a resolvable re-export returns early —
  so the declaration branch below it was never reached. The question is now asked of `masked_code`,
  where a string holds no keywords.
- **A reserved word is a legal property name.** `reject_surviving_esm` flagged `export`/`import` as
  a token anywhere in the module, which made it strictly broader than the rewriter it was checking.
  A `.` before or a `:` after is a position no statement can occupy.
- **A generator's `*` binds to the keyword.** Both graphs listed the declaration forms with a
  trailing space, so no generator matched. The Rust helper `extract_declaration_name` had known
  about `function* ` all along; only the dispatcher above it did not.
- **The linker wants one statement per line.** A `.ts` module always gets that from the transform,
  but `.js`/`.mjs`/`.cjs` is passed through, and the normalization only covered statements _sharing_
  a line — the minified direction. Prettier produces the other one the moment a list outgrows the
  print width. `has_esm_clause_spanning_lines` now covers it, restricted to the clause form so
  `export default {` and `export const x = {`, which the linker spans deliberately, keep their exact
  bytes.

`crates/ruvyxa_bundler/src/linker.rs` grew an adversarial suite that links fifteen shapes and
**parses the result**, because "the bundle does not parse" is the failure these produce;
`tests/packages/ruvyxa/compiler.test.mjs` imports and runs the rewritten module for the Node half.

### `ruvyxa dev` compiled server modules into the browser bundle

**Fixed.** A module's lane — client, server, action, or shared — decides whether it may appear in a
browser bundle. The Rust bundler read it from the leading directive and then from the file stem, so
`'use server'`, `server.ts`, `server.js`, `server.mjs`, `action.ts`, and `actions.ts` were all
server-side. The Node module graph matched one literal filename: `server.ts`.

Everything else on that list therefore compiled cleanly into the client bundle under `ruvyxa dev`
and the SSG render path, and was refused by `ruvyxa build` and `ruvyxa check` with
`RUV1820 invalid client to server module crossing`. A project following the documented convention in
JavaScript rather than TypeScript — `app/products/server.js` — had **no** client-boundary protection
in development at all, and neither did any action module in any language. Server-only source was
served to a browser in the one environment where a developer is looking at the page, and the error
arrived at the end of the cycle instead of the start.

`moduleLane()` in `packages/ruvyxa/runtime/compiler.mjs` now reads the directive and the file stem
the same way, and `tests/fixtures/module-lane-conformance.json` holds both graphs to one table —
which directive marks which lane, which stems mark which lane, and which crossings a client bundle
may not contain. The action-imports-client crossing stays the Rust bundler's alone, because this
graph has no hook on the server compile that could see it, and the fixture says so rather than
implying a parity that does not exist.

**If `ruvyxa dev` now reports RUV1007 on a file it accepted before**, that import was already
failing `ruvyxa build`. Move the server work behind a route handler or a server action and pass
serializable data to the client.

### A retired worker is guaranteed to stop

A worker being replaced is drained rather than killed, so an API stream still in flight finishes.
That wait had no ceiling, and the task draining it held the last `Arc` to the process: one request
that never reached a terminal frame kept a whole Node process — and its module graph — alive for the
life of the server. `recycle` retires a full generation on every instrumentation change, so those
accumulated. The drain now has a 60-second ceiling and shuts the process down either way, logging
what was still pending.

One request could produce exactly that stranded state. `send` answers single-frame requests, so the
stdout reader removes the pending entry only when the frame it sees is terminal — and a worker that
replied with a non-terminal frame left an entry whose receiver had just been dropped, which
`wait_until_idle` can never observe as idle. `send` now clears its own entry on every path; removing
one the reader already took is a no-op.

### The generated-entry preludes and route composition are gated end to end

`entry-prelude-parity.test.mjs` now covers the metadata prelude as well as the routing context and
the error boundary: both copies are executed against one stand-in React and asked the same questions
about merge order, `titleTemplate` depth, the `og:`/`twitter:` block, and the server-only
`<html lang>` rewrite. Metadata decides every page's `<head>`, and the two copies were held together
by a doc comment.

Route composition is held by `tests/fixtures/entry-composition-conformance.json`, which carries the
exact source each bundler emits for a route's tree and its loading shell. Order is the whole
contract — the boundary inside the Suspense so a synchronous throw renders its own UI rather than
the loading fallback, the layouts wrapping outward, the metadata a sibling rather than a wrapper —
and a bundler that reordered any of it still produced a working-looking page, on half the
deployments.

### The worker protocol is its own module

`crates/ruvyxa_dev_server/src/worker_protocol.rs` holds the NDJSON request and response types — the
wire format `packages/ruvyxa/runtime/worker-pool.mjs` reads and writes, where every field name is a
cross-language contract. It sat among the pool's scheduling, timeout, and recycling policy in a
3,365-line file. `worker_pool.rs` keeps the policy and the worker process it manages; those two
change together and stay together.

### The bootstrap block escapes its own payload

`bootstrap_data_block` took the route parameters and the request path as already-serialized,
already-escaped strings. Four writers across three modules called it, all four escaped correctly —
and the signature accepted an unescaped string just as readily. These values come from the request
URL, so one writer that forgot would let a path segment close the `<script>` element. That writer
had already existed once: the CSR shell interpolated raw `serde_json` output, which escapes `"` and
`\` but not `<`.

It now takes the values themselves and does the serializing and the escaping, assembling the payload
as a JSON document rather than formatting it as text. There is no longer a way to reach the element
unescaped, and `inline_script_json` in the CLI's prerender writer — which existed only to apply the
escaping before handing the result on — is gone. The escaping test passes the dangerous value
straight in, in both the parameter and the path position.

### `ruvyxa_dev_server`'s crate root is half its former size

`lib.rs` held 4,973 lines and had taken more commits than any other file in the repository:
configuration, `serve`, the router, the development file watcher, three WebSocket endpoints, ten
HTTP handlers, and the tests for all of it. The crate already had twenty-three focused modules
beside it, so the seams existed and `lib.rs` was what had not been moved through them.

Three modules came out of it, each with the tests that pin it:

- `watcher.rs` — the development file watcher and the HMR updates it produces: the watch loop, what
  a changed path means, the wire payload, and the rules for which paths are worth waking up for.
- `framework_endpoints.rs` — the handlers behind the reserved `/__ruvyxa/*` paths. This is the same
  surface `tests/fixtures/framework-endpoint-conformance.json` lists, so the module and the contract
  now describe the same thing.
- `realtime_endpoints.rs` — the HMR, realtime, and presence WebSockets, with the origin check,
  channel validation, and subscription filter that decide what a socket may see.

`lib.rs` is 3,262 lines and keeps what a crate root should: configuration, `serve`, the router
table, and the request path for project pages and API routes. Its implementation half went from
2,973 lines to 1,561. Behaviour is unchanged — the same 298 tests pass before and after, and no
handler, route, or public export moved.

### The generated-entry preludes had no gate

`crates/ruvyxa_bundler/src/output.rs` said `tests/packages/ruvyxa/entry-templates.test.mjs` asserted
that its routing-context and error-boundary preludes agreed with
`packages/ruvyxa/runtime/entry-templates.mjs`. That test never read `output.rs`. The two bundlers
each emit a route's client entry, so the preludes were two hand-maintained copies of the same source
text with nothing but a doc comment between them — and a drift would have shown up as a boundary
that swallowed a `notFound()` in one build and rethrew it in the other.

`tests/packages/ruvyxa/entry-prelude-parity.test.mjs` executes both copies against one stand-in
React and asks them the same questions: what `getDerivedStateFromError` returns, what a `notFound()`
signal renders with and without a `not-found.tsx`, what the error fallback receives, and what
`retry()` does with and without a mounted router. Behaviour rather than bytes, because the Rust
literal carries statement terminators the Prettier-formatted template does not.

### Plugins can tell development from production

`register(api)` receives `api.environment`. A host that does not state one reports `production`, so
development-only behaviour is never enabled by omission, and the native server passes it explicitly
rather than inferring it from `watch` — a development server with watching disabled is still a
development server.

It is available at registration rather than per request on purpose: a plugin that only makes sense
in one environment declines to register its hooks at all, so the other environment pays nothing, not
even a comparison.

`feed` and `searchIndex` use it. Given a loader rather than a static list, they now answer requests
in development, where there is no built asset to serve; in production they stay build-time only,
because running a project's loader per request would put a file read or a database query on the
response path in front of the file the build already wrote. Their development answers are
deliberately not memoized: the developer edits the source the loader reads.

`createPluginHarness` takes `{ environment }` so both branches are testable.

### Five plugins join `ruvyxa/plugins`

`originGuard`, `healthCheck`, `webVitals`, `llmsTxt`, and `wellKnown` are exported from
`ruvyxa/plugins` alongside the existing catalog. Each is built from the public plugin API, so a
project can compose them with its own plugins or reimplement any of them.

`originGuard` is the one worth reading twice. Server actions already reject cross-site requests in
both hosts — `action-runtime.mjs` and `action_security.rs` — but a handler under `app/api/` gets
none of that: it is reachable from any origin, and a session cookie defaults to `SameSite=Lax`,
which a cross-site form POST still carries. The plugin compares `Origin` against `Host` for unsafe
methods, accepts `Sec-Fetch-Site: same-origin` when the origin was stripped, and fails closed when
neither is present. It does **not** weigh `X-Forwarded-Proto` the way the action guard does: that
comparison is only meaningful against a trusted-proxy list, which a plugin cannot reach. The host
comparison is the load-bearing check either way.

It is opt-in rather than a default. An API meant to be called from another origin is a legitimate
design, and that case is governed by CORS.

`webVitals` publishes its client script as a build asset and loads it with `src` rather than
inlining a snippet into `<head>`. An inline snippet would force `'unsafe-inline'` into every
`script-src` policy that wanted the plugin, so the plugin that measures performance would have
quietly cost the application its CSP. Its collector accepts only the shape its own script sends —
the endpoint is reachable by anyone, and an unvalidated payload would let a third party write
arbitrary strings into the application's logs.

### Generated files are served during development

`robots`, `feed`, and `searchIndex` only ever wrote their output at `build.onComplete`, so
`/robots.txt`, `/rss.xml`, and the search index answered 404 under `ruvyxa dev` — the output could
not be checked without a production build. They now answer requests for their own file from the same
bytes the build writes, which is what `openApi`, `pwa`, and `contentEngine` already did.

`feed` and `searchIndex` do this only when their content is a static array. Given a loader they stay
build-time only, deliberately: a plugin cannot tell development from production at request time, so
running a loader per request would put a file read or a database query on the production response
path — and shadow the asset the build already wrote with a different one. `sitemap` and the new
`llmsTxt` remain build-time only because their entries come from the route manifest, which does not
exist while the development server is running.

### `plugins.ts` is a barrel

`packages/ruvyxa/src/plugins.ts` was 2,954 lines covering every first-party plugin family. It is now
a barrel that re-exports the public API by name from `packages/ruvyxa/src/plugins/`: one module per
family (`http`, `pwa`, `seo`, `search`, `content-engine`, `openapi`, `build`), plus `shared` for
helpers two or more families use and `sitemap-xml` for the sitemap document builder.

The split follows the section markers the file already carried, so it reflects the boundaries the
author drew rather than new ones. All 145 declarations moved unchanged — none lost, added, or
duplicated — and the built module exports the same 17 functions it did before.

The barrel lists names explicitly rather than re-exporting `*`, because the family modules also
export helpers to each other and a wildcard would publish those as package API. A new plugin goes in
its family module and gets one line in the barrel; adding it only to the module leaves it
unreachable from `ruvyxa/plugins`.

### `RenderMeta` has one source of truth for client scheduling

`RenderMeta` carried both a `hydrate: bool` and a `hydration: HydrationMode`, where the boolean was
only ever `hydration != None`. Both were public and independently assignable, and code in the tree
already set one without the other — so the bundler and the document writer could disagree about
whether a route ships JavaScript. The boolean is gone and `RenderMeta::ships_client_bundle()`
derives the answer. The `export const hydrate = false` API is unchanged.

### Dependencies

`oxc` moves to `0.146.0` on both sides of the lockstep — the Rust bundler and the `oxc-transform`
the Node compiler uses — because a version split lets one page's server render and client hydration
disagree. `MinifierOptions` gained a field in that release; the options literal now spreads the
defaults and names only the field it means to change, so the next addition is not a compile error
that says nothing about what the new option should be.

Also updated: `@babel/core` to 8, `pnpm` to 11.22.0, `oxlint` to 1.79.0, `sass` to 1.103.0, and the
usual transitive movement. `babel-plugin-react-compiler` still emits memoisation and source maps
under Babel 8.

`@types/node` is deliberately held at `24.13.3`, the newest release on the 24 line. `engines.node`
is `>=24.19.0`, and types from a later major would let code typecheck against APIs that do not exist
on the runtime the package tells users to run.

Oxlint 1.79 brought rules that found real defects rather than noise. The one worth naming: `<Link>`
tracked whether it had prefetched with a boolean plus an effect that reset it when `href` changed,
which left a window — between the href changing and the effect running — where the guard still said
"already prefetched" and the new destination was skipped. The guard now records _which_ href was
warmed, which closes the window and removes the effect. A ref written during render in
`@ruvyxa/realtime` was removed as well: it could not stabilise a dependency that sat beside it in
the same list.

## v1.0.31 (2026-08-20)

### Node.js 24 LTS production baseline

- Raised the root, published package, demo, and starter-template Node.js minimum to `>=24.19.0`.
- Decoupled `@types/node` patch numbering from the runtime floor while keeping every package on the
  same latest Node 24 type contract (`24.13.3`).
- CI now verifies Node.js 24.19.0 across every supported operating system; security and release jobs
  use the same exact version.
- AWS Amplify, Firebase, Render, and Vercel deployment output now defaults to the Node.js 24 line.
  Render constrains automatic updates to `>=24.19.0 <25` so patch updates do not cross a major.

### Server actions, plugin HTTP hooks, and `security` reach deployed builds

Ruvyxa has two request hosts: the Axum server behind `ruvyxa dev` and `ruvyxa start`, and
`createHandler`, which every adapter's function artifact and the generated standalone server run.
Endpoints were added to the Axum router by hand and ported to the handler one at a time, and nothing
checked that the two agreed. Three things had never been ported.

- **Server actions.** `POST /__ruvyxa/action` existed only in the native host, so it fell through to
  route matching and returned 404 in every deployed build — every form in the `crud` template, in
  `examples/demo/app/todos`, and in the markup `ruvyxa add` generates. The endpoint, its CSRF and
  payload rules, and its rate limit now run in both hosts from one implementation
  (`runtime/action-runtime.mjs`), and `adapter-runner.mjs` compiles each route's `action.ts` into
  the function bundle. A build without an action registry answers `501 RUV2211` rather than 404, so
  a misconfigured deploy is distinguishable from a project that declares no action.
- **Plugin HTTP hooks.** `plugin-runtime.mjs` is spawned only by the Rust host, so `http.onRequest`,
  `http.onResponse`, and `http.route` did nothing once deployed — including all of `@ruvyxa/auth`,
  whose entire surface is one `http.onRequest` registration, and the built-in
  redirect/headers/rewrite plugins from `ruvyxa/plugins`. The registry moved to
  `runtime/plugin-http.mjs`; the Rust host still reaches it over stdio, and a function bundle now
  compiles it in and runs the same hooks against native `Request`/`Response` objects.
- **`security`.** `runtimeBuildPolicy()` returned only `buildInfo.runtime`, so the validated
  `security` block was dropped: a deployed function had **no request body limit at all**,
  `security.headers: false` had no effect, and `security.trustedProxyIps` was unused, while
  `ruvyxa start` enforced all three. All three are now honoured, with the body cap enforced on the
  bytes read rather than on a `Content-Length` the platform may not provide.

Two mechanisms exist so this cannot recur silently:

- `tests/fixtures/framework-endpoint-conformance.json` records every framework endpoint and which
  host must serve it, replayed by a Rust test and a Node test. Add the endpoint there first.
- `ruvyxa check` and `ruvyxa test:parity` gained a capability axis. The route sweep compared the
  development app directory against the built one — two inputs to the same renderer — and never
  asked whether the other host could serve the project at all.

`ruvyxa build` now fails rather than emitting a deployment that answers 404: a static adapter with a
server action or a plugin HTTP route reports `RUV2204`. Realtime and presence need a socket upgrade
no build artifact can perform; that reports `RUV2205` as a warning, because a deployment without
that endpoint is still a valid deployment.

Selecting a target with `--adapter <name>` now also loads `ruvyxa.config`, which is where `plugins`
live and therefore where the function bundle's plugin registry comes from.

### Server-action replay protection fails closed

The versioned-action replay guard was a `BTreeMap` of nonce to expiry, and every action paid for it:
the `retain` sweep walked all 10,000 entries even when the first was still live, and a saturated
guard then scanned again for the minimum to evict. It is now two structures over one set of keys —
`seen` answers the replay question and `order` holds the same keys in expiry order. Every nonce
shares one TTL, so insertion order is expiry order and the sweep stops at the first live entry:
replay detection and expiry sweeping are both O(1).

Saturation now fails closed. At the entry cap the old guard evicted the oldest live nonce to make
room — and because every entry shares one TTL, the oldest tracked nonce is also the one with the
most time left to live, so evicting it _accepted its replay_, the one thing the guard exists to
refuse. An attacker reaches that state on purpose by sending `maxEntries` fresh nonces. Both hosts
now answer `503` instead, pinned by the `saturation` clause of
`tests/fixtures/action-contract.json`.

The follow-up closed the remaining hole: the rate limiter in front of the guard is keyed per client
_and_ per path and action, so one client spreading requests over two actions earned two fresh
buckets while the nonce pool stayed one — enough for a single address to saturate the pool alone and
refuse every other client's actions for a TTL. Each address may now hold at most a tenth of the pool
(`ACTION_NONCE_MAX_PER_CLIENT`, 1,000 of 10,000), tracked in a per-client count swept together with
`order` and dropped when the address's last nonce expires. An address over its own share is refused
`429` — its problem, not the service's — while the pool still serves everyone else; reaching global
saturation now takes ten distinct addresses. The quota and the rate limiter share one
client-identity function, so forwarded identity (trusted only from a loopback or allowlisted peer,
never merely a private range) cannot be attributed one way by one check and another by the other.

Rejections also stopped being stringly typed. `handle_action` used to recover the HTTP status by
comparing the guard's message against string literals copied at the call site, so rewording a
message silently answered `400` where the fixture pins `503` — a drift no test could catch, because
the fixture's status was replayed by the serverless suite and by nothing on the Rust side.
Rejections are now an `ActionReplayRejection` enum that carries its own status (`400` invalid, `409`
replayed, `429` client quota, `503` global saturation), and both hosts' suites replay the fixture.

### A regular expression no longer hides the `require()` calls after it

The linker rewrites codegen output line by line, and its `require()` and dynamic-`import()` passes
each carried their own walk over the bytes. Both knew about strings and comments; neither knew about
regular expressions.

- `/[/*]/` — a character class holding a slash and a star — read as a block-comment opener, and that
  state is carried between lines, so **every following line of the module was swallowed as comment
  text**. Nothing after it was rewritten.
- `/"/g` — the shape of `str.replace(/"/g, …)` — opened a string that never closed, hiding every
  `require()` later on the line. Minified CommonJS puts a whole module on one line, so that is every
  require in the file.

Both passes now ask `ast::skip_non_code`, alongside the crate's one scanner, which handles comments,
strings, template literals, and regular expressions together — the combination `regex_can_start`'s
own documentation says the decision requires. Neither pass carries a private walk any more, and
`advance_char`, which existed only to serve them, is gone.

The same walk also reached a third defect. A template literal used to be skipped whole, so
`` `built with ${require("pkg").name}` `` kept its `require()` — while the dependency scanner, which
does read `${…}` as code, had already put `pkg` in the graph. The module was bundled and the call
site still said `require`, which is a `ReferenceError` in a browser bundle. Interpolations are now
walked by the pass that walks the statement around them; template _text_ is still data.

### A panic no longer ends collaboration for the life of the process

`CollabRegistry` took its lock with `.expect("collab registry poisoned")` at all five call sites, so
a single panic while the lock was held made every later join, presence update, write, and leave
panic in turn — peers could not even leave a room. Nothing under that lock spans two fields, so the
state behind a poisoned lock is as valid as the state behind a healthy one; the registry now
recovers the guard and keeps serving.

### HMR gained lanes, versions, and a browser client that stops reloading for everything

Development dependency tracking is now kept per lane. `HmrTracker` maintains file-to-route reverse
dependencies separately for the manifest, server, client, and action lanes, so a change to a server
module no longer invalidates client work that does not depend on it, and a server action carries its
own dependency set.

The wire protocol is versioned. Every message names `ruvyxa.hmr`, carries a monotonic `sequence`,
affected module and route identifiers, and one of `partial`, `restart`, or `issues`. The inline
browser client rejects any message whose sequence it has already applied, so a superseded update can
never land after the one that replaced it. A CSS change now replaces the affected stylesheet in
place rather than reloading; anything that cannot be proven safe still falls back to a full reload,
which remains the correct answer rather than a failure.

`tests/fixtures/hmr-contract.json` records the message shape and the stale-message policy, replayed
against the payload builder so a field or event rename cannot pass unnoticed.

The superseded pre-versioning fixture, `hmr-legacy-contract.json`, has been removed. It described a
wire shape (`css-update`, `component-update`, `full-reload`) that the versioned protocol replaced,
nothing in either language replayed it, and the skew it claimed to guard against cannot occur: the
browser client is a string inlined into the HTML by the same dev-server process that sends the
messages, so client and server always ship together.

### Build artifacts have one identity, dependency, and eviction contract

`ruvyxa_bundler` gained an internal artifact task graph. Compiler output, resolved edges, chunk
plans, and emitted artifacts keep living in their own caches; the graph gives them one typed
identity (`ArtifactKey`, derived from length-framed, name-sorted semantic inputs so callers cannot
change a key by iterating a map in a different order), one lifecycle (`Building`, `Ready`, `Failed`,
`Cancelled`), dependency edges, generation-scoped cancellation, and atomic persistence. A graph hit
is never treated as artifact bytes: callers still validate and load the owning cache entry, and a
corrupt or incompatible manifest is a plain cache miss.

Two builders that publish different content for one semantic key now fail closed rather than letting
whichever finished last win, and an artifact that completes after its generation was invalidated is
rejected as stale.

A shared cache budget sits over the compiler, resolver, and artifact caches, with the same
hysteresis policy implemented in Rust and in the worker runtime and held to
`tests/fixtures/cache-budget-contract.json`. Memory pressure never changes output semantics — the
worst legal result is a slower rebuild — and an artifact owned by an in-flight build is pinned by
its state and its dependency edges for as long as the build holds it.

`ruvyxa bench` gained reproducible cold-build, warm-build, and first-route scenarios that clone
project inputs into a private temporary workspace with its own cache, and verify that cold and warm
builds emit the same artifacts before publishing any timing.

### `import.meta.glob`

The resolver expands literal `import.meta.glob` calls at compile time, so the bundler analyses,
chunks, caches, and invalidates every match. Patterns and options must be compile-time literals;
anything else is a diagnostic rather than a runtime fallback, and a pattern that escapes the project
root is rejected.

Both module graphs expand it, and getting them to agree exposed three defects:

- **Eager matches were unusable outside the Rust bundler.** Eager lowering emitted
  `require(specifier)`. The Rust linker rewrites `require()` into a bundled binding, so it worked
  there; the JavaScript compiler has no such pass, so the call reached an ES module and threw
  `require is not defined` at runtime. Eager matches now lower to hoisted namespace imports in both
  graphs, which is also what puts them in the static dependency graph as documented.
- **Generated imports had nowhere safe to go.** Appending them left the linker's rewritten `const`
  binding in the temporal dead zone of every earlier use; prepending them displaced a `'use client'`
  directive, which is only a directive while it is the first statement and silently becomes a plain
  string expression otherwise — taking the whole server/client boundary check with it. They are now
  inserted after the directive prologue, computed by one helper per language.
- **The two graphs ordered keys differently.** Rust sorted matches by code units and JavaScript by
  `localeCompare`, so `B.ts` came first in one and `a.ts` in the other; `localeCompare` also varies
  with the host ICU locale, so the same project did not build the same way on two machines. Both now
  compare code units.

`tests/fixtures/glob-contract.json` is at schema version 2 and is replayed by both languages.
Version 1 declared only cases with zero or one match, so it asserted the word "deterministic"
without ever exercising an order — which is why the ordering split survived. It now pins key order,
eager lowering, and the scanning rules, with a case whose filenames differ by more than case so it
also runs on case-insensitive filesystems.

### One source scanner on the JavaScript side

`packages/ruvyxa/runtime/scanner.mjs` is now the only JavaScript-side source scanner, ported from
`crates/ruvyxa_bundler/src/ast.rs`.

Glob expansion had shipped its own walk over the source, and it did not know about regular
expressions. A literal such as `/['"]/` starts a string skip that runs to the next quote anywhere in
the file, so a `import.meta.glob` call after one was never seen — and the failure was silent:
`import.meta.glob(...)` was emitted verbatim into the output instead of raising a diagnostic. This
is the same failure class that was fixed at the root in Rust by making `ast.rs` the only byte
scanner; the JavaScript graph had never had the equivalent, so every new text transform there
started by writing a second scanner.

The shared module handles comments, strings, template literals and their interpolations, and regular
expressions together, including the character-class state that decides where `/[/"']/` ends. Route
any new JavaScript-side text walk through it.

### `paths` now honour a `baseUrl` inherited through `extends`

TypeScript resolves `compilerOptions.paths` against the effective `baseUrl`, including one inherited
from an extended configuration, and falls back to the declaring file's directory only when no
`baseUrl` is in effect. Both Ruvyxa resolvers used the declaring file's directory unconditionally,
so a base config that supplied `baseUrl` had it silently ignored by any child that declared `paths`
— and because both graphs were wrong in the same direction, no parity fixture caught it. The editor
and the type checker resolved those imports one way and the bundler another.

`tests/fixtures/path-alias-contract.json` gained the case, replayed in both languages.

The pattern-precedence tiebreak in the JavaScript resolver also moved from `localeCompare` to code
units, matching `alias_pattern_order` in Rust. This one was not reachable — two patterns of equal
specificity can never both match one specifier, because equal literal prefix and suffix lengths
force the patterns to be identical — but it is one less locale-dependent comparison in a resolver.

### Cache eviction is no longer quadratic

Evicting artifacts rescanned every record in the graph to pick each next candidate. Measured in
release mode on one machine, evicting a full graph took 8.75ms at 500 records, 194.75ms at 2,000,
and **4.41 seconds at 8,000** — work that lands precisely when the process is already short on
memory.

Eligible records are now kept in a priority-ordered set that is repaired incrementally: only the
dependencies of an evicted record can newly become eligible. The same three sizes now take 0.83ms,
4.38ms, and 20.59ms, and eviction order is unchanged — discarded work first, then least-valuable
ready artifacts, ties broken by artifact key.

Eviction also now measures the same quantity the budget controller accounts for. It compared a
target derived from evictable bytes against a total that included the pinned closure of an in-flight
build, so a build holding a large pinned closure made up the difference by discarding healthy
`Ready` artifacts — rebuilds the budget never asked for.

`ArtifactTaskGraph::evictable_bytes` replaces a full `stats()` call on the per-route budget path,
which had been recomputing dependency-edge totals, state counters, and a second residency pass that
no caller of that number reads.

### Unused-code detection is a release gate

`pnpm check:unused` runs Knip across the JavaScript and TypeScript workspaces and fails on unused
files, exports, types, and dependencies. `pnpm release:validate` runs it too.

Ruvyxa loads a great deal of code by convention rather than by import — `app/` routes, `plugins/`,
`ruvyxa.config.ts`, the `runtime/*.mjs` modules the Rust CLI resolves by path, and adapters resolved
from a `@ruvyxa/adapter-${name}` template string. `knip.json` declares those, which took the report
from 102 false positives to zero. A dependency audit against it found no genuinely unused
dependency.

It immediately found one real defect: `@ruvyxa/core` exported `SiteConfig` as public API while its
`sitemap` and `robots` fields were typed `SiteSitemapConfig` and `SiteRobotsConfig`, neither of
which was re-exported. Consumers received a public type referencing names they could not import.
`SiteSitemapConfig`, `SiteRobotsConfig`, and `SiteRobotsRule` are now part of the public surface.

Knip must stay on version 6 or newer; version 5 crashes against this repository's TypeScript 7.

### A request path resolves the same in both hosts

The static-asset and prerendered-document path check was the latest host split. The Rust side
checked with `Path::components`, which accepted `foo:bar` (only a single-letter `a:` parses as a
Windows drive prefix, so multi-character names slipped through), accepted control characters, and
folded a bare `.` away as a current-directory component — while the deployed handler's
`isUnsafeSegment` rejected all three, so one URL resolved differently under `ruvyxa start` than in a
deployed build. The rule guards a path that is written as well as read, and on Windows `foo:bar`
names an NTFS alternate data stream, so the split was a write-path hole, not just a 404.

Both hosts now check the same explicit segment rules — no `.` or `..` segment, no `/`, `\`, `:`, or
control character — held to `tests/fixtures/prerender-path-conformance.json`, replayed by a Rust
test against `is_safe_relative_path` and a Node test against `prerenderRelativePath` in the deployed
handler. The fixture's cases include non-ASCII segments and dots inside a segment, so the two can
drift only by rewriting the table.

### Environment access is a cross-language contract

The private-env rule — a `process.env` read is private unless the name is exactly `NODE_ENV` or
begins with `RUVYXA_PUBLIC_` — was held by a comment promising both languages agreed, and the rule
had drifted once before. `tests/fixtures/env-policy-conformance.json` now pins it with a case per
edge: `NODE_ENVIRONMENT` is private because the exemption is an exact match, `RUVYXA_PUBLIC` is
private because the prefix keeps its trailing underscore, `node_env` is private because the
exemption is case-sensitive, and an empty name is not an exemption. Both
`boundary::env_read_is_private` (Rust) and `envReadIsPrivate` in `runtime/compiler.mjs` replay the
table, and the Node suite also asserts the scanner calls the predicate rather than keeping an
inlined copy of the comparison — so the fixture cannot pass while the product checks something else.

The commit that built the fixture also fixed the identifier-boundary check in the Rust AST parser,
which used `saturating_sub` and so clamped the boundary at offset zero — a module opening with an
`export` statement could fail named-export detection. `checked_sub` surfaces the underflow instead,
and tests cover exports at offset zero plus the look-alikes that must be rejected.

### `String.replace` replacement strings are data, not patterns

`String.prototype.replace` interprets `$&`, `` $` ``, `$'`, and `$1`-style sequences in a
replacement _string_. Five rewrite sites used replacement strings where the value came from project
configuration or page content: the lang-attribute injection in `__ruvyxaApplyLang`, PWA manifest and
register-tag injection, title-template wildcard resolution, tsconfig path rewriting, and font-URL
`publicPath` rewriting. A configured value containing `$&` would silently substitute the matched
text back into the output — attacker-controllable wherever the configured path is itself
attacker-influenced. All five now use replacer functions, whose return value is always literal text;
tests pin `$&` in a manifest path and `$-substitution` characters in a lang value.

The two commits that swept these sites also consolidated realtime event validation into
`runtime/action-runtime.mjs` — one implementation for the Rust host and the serverless handler,
which had diverged — and added that module to the artifact-cache invalidation list so prerendered
output follows rule changes.

### The render export is validated before it is called

On Windows CI and under high parallelism, an isolated module import can race `writeIfChanged` and
evaluate a partially written output file that lacks the expected `render` export — an empty module
where a page should be. `importRenderModule` now asserts `mod.render` is a function before it is
called, on both the SSR and SSG paths; on failure it evicts the broken module-cache entry, waits
briefly for the filesystem to settle, and re-imports once. If the retry still fails, the diagnostic
lists the exports that were actually present instead of the bare `TypeError` a call would produce,
so a bundler or linker failure is visible in CI logs rather than masquerading as a render error. The
one-shot `ssr-renderer.mjs` used by `ruvyxa test:parity` and tooling fails with `RUV1100` and the
same export list.

### Both languages share one lint gate

`pnpm lint` runs Oxlint across `packages/`, `templates/`, `examples/`, `scripts/`, and `tests/`,
with correctness, suspicious, and performance rulesets at error level. Every rule the configuration
turns off carries its reason beside it — sequential `await` in a loop is how the bundler applies
backpressure, `void` before a floating promise is the marker TypeScript itself recommends — so a
rule can be disabled only with an argument, never to clear a finding. `localeCompare` is banned
outright: ordering decides cache keys, content fingerprints, and the bytes of files the build
writes, and two machines building the same project disagreed. The ban is enforced by lint, with
`compareCodeUnits` / `compareEntryKeys` from `runtime/order.mjs` and `compareStable` from
`src/plugins.ts` named in the error.

The Rust side got the matching gate: `.clippy.toml` sets `cognitive-complexity-threshold` and the
workspace lint turns it on, so a Rust function that grows past what one screen holds fails
`cargo clippy -- -D warnings`. The enforcement wave also added structural caps on the JavaScript
side — complexity 30, max-depth 4, max-nested-callbacks 4, max-params 8 — and the refactor those
caps forced out of `ruvyxa_middleware` and the runtime modules fixed CORS header placement along the
way.

`Allow-Methods`, `Allow-Headers`, and `Max-Age` answer a preflight question, and the Fetch standard
has browsers read them only from a preflight response. Sending them on every response advertised the
whole method and header allowlist to any origin that got a response at all, and invited a proxy to
cache a `Max-Age` that was never negotiated. `Allow-Origin`, `Allow-Credentials`, and `Vary` belong
on both, because the browser checks those on the actual response too. `apply_cors_headers` now takes
a `preflight` flag, mirrored by `withCorsHeaders` in the serverless handler so the two hosts cannot
split again, and the middleware test pins exactly which headers cross the line.

### Image builds are bounded by one encode

Public-image optimization dropped the `image` facade for the decoders it wraps. `image` declares
`avif`/`exr` as optional features, and Cargo records a dependency's optional deps in the lockfile
whether or not the feature is on — so `ravif`/`rav1e`/`pulp` and the unmaintained `paste` macro
(RUSTSEC-2024-0436) sat in `Cargo.lock` permanently even though none of them ever compiled. The
build now calls `png`, `zune-jpeg`, and `image-webp` directly, and `fast_image_resize` — which
carried the same optional `image` dependency — was replaced by `pic-scale` for the same SIMD
convolution: measured on the same 6000x4000 ladder, 72 ms with `fast_image_resize`, 89 ms here, the
price of a lockfile with no unmaintained crate in it. A first attempt to enforce
`cargo audit --deny warnings` needed an ignore for that advisory; the change was reverted, and the
cleanup then removed the advisory's subject entirely.

Decoding was reworked alongside. `decode_within_pixel_budget` used to sniff the magic bytes and
parse the header for the budget check, and the decoder then parsed it again — three passes over the
same prefix, invisible on a 24 MP photo and most of the per-image cost on a directory of icons. The
budget check now lives inside each decoder, against the reader it has already built, and a JPEG
declares its dimensions before its pixel buffer is allocated — an oversized image is refused before
allocation rather than after. The error type became an enum, `Unsupported` /
`TooLarge { width, height, max_pixels }` / `Malformed`, because the runtime image endpoint answers
`413` for the budget case and `400` for everything else and used to recover that distinction by
re-parsing the header a second time at the call site — the same stringly-typed coupling removed from
the action replay guard. Resize results are taken out of the store instead of copied —
`borrow().to_vec()` was copying every output a second time, 29 MB per 3840-wide variant and one
source emits eight of them — grayscale widening now writes through fixed-width chunks so the loop
vectorizes, and the resizer runs `Adaptive` threading: the ladder is 71 ms adaptive against 156 ms
single, the full build 691 ms against 704 ms, so the nested pool costs nothing while `Single` halves
the runtime endpoint that resizes one image per request. libwebp's `thread_level` re-measured as a
loss (750 ms with it off, 814 ms with it on), and `effort` is where the time is: 0 → 221 ms / 225
KB, 2 → 323 / 205, 4 → 752 / 197, 6 → 943 / 197 — so the build default stays at 4 and the request
path, where latency is what a user feels, uses 2.

Then the critical path was removed. The primary output used to be encoded at the source's own
resolution, and because libwebp cannot split one lossy encode across cores, that single job set the
wall time of the whole build — 745 ms of a 6000x4000 build was the full-size encode of a file no
viewport can use. `image.maxWidth` (default 3840, the top of the standard responsive ladder and the
width of a 4K display) caps the primary output before encoding, and that build drops to 296 ms; `0`
restores the uncapped behavior for projects that publish full-resolution originals on purpose. The
manifest reports the width that was emitted rather than the one the source held, variant widths are
filtered against the capped primary so nothing duplicates it, and the cache key accounts for the
resize.

Decode coverage grew with the rework: palette and interlaced PNGs and CMYK JPEGs are now exercised,
and the solid-colour resize test became a tolerance check after CI saw `202` for `200` on a target
whose AVX2, NEON, and scalar paths round the same fixed-point Lanczos weights differently — a drift
of a couple of levels is the arithmetic, while the defect the test guards against moves whole
channels.

### Documentation is where the implementation is

`roadmap.md`, a 598-line modernization roadmap, was deleted: its proposals had been absorbed by the
implementation, and its decision notes belonged with the architecture document, which gained the
glob expansion, caching, and HMR sections the entries above describe. The crate list now counts
`ruvyxa_tui`, and the API reference covers the environment-policy enforcement rules.

## v1.0.30 (2026-08-14)

### Global CSS runs through the project's PostCSS chain

If the project root has a PostCSS configuration, Ruvyxa now runs that plugin chain over every
collected global stylesheet, in `ruvyxa dev` and `ruvyxa build` alike, on one code path. A Tailwind
CSS v4 project needs `postcss.config.mjs` and `@tailwindcss/postcss` and nothing framework-specific;
before this, the stylesheet was emitted with `@import "tailwindcss"` still in it, which a browser
cannot resolve, so the page rendered with browser defaults while the markup carried correct class
names.

- Recognised at the project root: `postcss.config.{mjs,js,cjs,ts,mts,cts,json}`,
  `.postcssrc.{mjs,js,cjs,json}`, `.postcssrc`.
- Ruvyxa names no plugin of its own. The config's plugins are resolved from the project's
  `node_modules`, in the array, `{ name: options }`, or function-of-context form.
- Plugins run per stylesheet entry, after this pipeline inlines local `@import`s, with `from` set to
  the real entry path so content globs resolve where the author expects.
- Files a plugin reads become watch inputs, so a dev edit that only changes class names regenerates
  the stylesheet. The config file itself is one too.
- A plugin failure fails the build (`RUV1406`) and a config that cannot be loaded fails with
  `RUV1405`. Ruvyxa does not fall back to untransformed CSS, because that ships an unstyled page.
- **A project with no PostCSS config is unaffected.** A stylesheet importing `tailwindcss` without a
  PostCSS config still falls back to `@tailwindcss/cli` when that is installed.

### JSON is a module kind, in both module graphs

Resolution answers which file, not which language. Without that split, a JSON file reached through
`require('./package.json')` — the shape `gaxios` uses, and through it `google-auth-library` and
`@google/genai` — was handed to the JavaScript transform, and every adapter build that bundled such
an SDK failed with a syntax error pointing inside a package the application never wrote.

- `import`/`require` of a `.json` file now compiles to data in the serverless/server graph
  (`runtime/compiler.mjs`) and the client graph (`ruvyxa_bundler`). A default import receives the
  whole document, as in Node; `require()` receives it unchanged, including a document with its own
  `default` key.
- The document is never scanned for imports and never folded for `NODE_ENV`, so a string value that
  looks like code stays a string value.
- Invalid JSON reports `RUV1805` naming the file and parse position, instead of an unrelated
  JavaScript syntax error.
- A resolved file whose extension has no compilation path — `.node`, `.wasm`, a binary asset —
  reports `RUV1806` naming the file, its extension, and the import that reached it, with
  `build.external` as the remedy.

Serverless adapters share one `bundlePackages: true` call site, so this covers every platform target
rather than the one that reported the failure.

### Real-time collaboration

Ruvyxa now ships collaboration rooms as a native transport rather than an integration you assemble.
`@ruvyxa/realtime` exports `collab()`, which claims the new `presence@1` capability and serves a
bidirectional WebSocket at `/__ruvyxa/collab`. The existing `realtime()` transport is unchanged and
remains send-only; the two are separate capabilities and a project may claim either or both.

```ts
import { config } from 'ruvyxa/config'
import { collab } from '@ruvyxa/realtime'

export default config({ plugins: [collab()] })
```

- A room carries **presence** (ephemeral per-connection state such as cursors, selections, and
  names) and **shared state** (retained for the room's life, last-writer-wins per key). Presence is
  dropped when a peer disconnects; shared state survives until the last peer leaves.
- The server is the only sequencer. Every accepted write takes the next room version, so "last
  writer wins" means "last frame to reach the process", no client clock is involved, and two peers
  writing one key converge on the same value. Shared state is **not** a CRDT: concurrent writes to
  one key replace rather than merge, so a document that needs concurrent edits to all survive should
  be split across keys.
- A joining peer receives a full room snapshot, so late arrivals never replay history. A peer that
  falls behind the room's broadcast buffer receives `resync` and reconnects for a fresh snapshot.
- `@ruvyxa/realtime/react` exports `CollabProvider`, `usePresence`, `useSharedState`,
  `useCollabRoom`, and `useCollabClient`. One provider owns one socket and hooks read it through
  `useSyncExternalStore`, so a room with many subscribers still holds a single connection. React is
  an optional peer dependency; `@ruvyxa/realtime/collab` exports `createCollabClient` for use
  without React.
- Outgoing presence is throttled into one trailing frame per window (`presenceThrottleMs`, default
  50 ms) so a cursor stream cannot exhaust the server's frame budget, and local presence is
  reflected immediately rather than a network hop late.
- Server-enforced limits: 64 peers and 256 shared-state keys per room, 1024 rooms per process, 32
  keys per write, 32 KiB per frame, and 120 frames per second per connection.
- Rooms are process-local and hold no storage. `collab()` fails the build with `RUV3201` on targets
  that are not long-lived Node/Bun output, and a deployment running several processes must pin one
  room's peers to one process.

### Content Engine `/llms.txt` is no longer experimental

- The `contentEngine()` agent discovery index is now a supported artifact with a stable output
  shape: an H1 title, a blockquote summary, and a single `## Content` section listing every
  non-draft page with its author-written answers. `llmsPath: false` still disables it.
- Page descriptions are now escaped in `/llms.txt` the same way titles and answers already were, so
  bracket and backslash characters from frontmatter can no longer inject Markdown link syntax into
  the index.

### Correctness: source scanning no longer trusts strings and comments

Three separate scanners were reading source text line by line and treating commented-out or quoted
constructs as real code. All three now analyze a masked copy of the source that distinguishes code
bytes from string and comment bytes.

- `ruvyxa_graph` route-export parsing shares that masked source through a new `export_const_value`
  helper. This fixes ISR and PPR opt-in being silently lost when the export carried a type
  annotation (`export const revalidate: number = 3600`, `export const ppr: boolean = true`), and
  stops commented-out exports and documentation strings from registering as real ones.
- The dev server's CSS scanner masks comment spans before collecting and removing `@import`
  statements, so a commented-out import is no longer followed. Builds previously failed when such an
  import pointed at a deleted file. Import collection and removal now share one mask, so the two
  passes can no longer disagree about which lines are code, and comment stripping operates on byte
  slices for correct UTF-8 handling.
- The bundler's AST string scanner stops at line boundaries when a quote is never closed, instead of
  consuming the rest of the file. An unbalanced apostrophe in a comment no longer swallows the code
  after it.

### Reliability

- `copy_dir_all` now refuses an output directory nested inside its source directory and reports
  `RUV1604` with an actionable message, instead of recursing until the build dies.
- The `observability()` plugin reports a zero duration when its response hook runs without a
  matching request hook, rather than deriving a duration from a missing header.
- The image optimizer serves an empty stylesheet in place of a missing Google Fonts sheet, so a font
  request cannot 404 a page that would otherwise render with fallback fonts.
- `cacheRules()` validates header values at config time by probing a `Headers` object, so an
  injection attempt fails during configuration rather than at request time.
- Forced-revalidation claim state is now observable: `RenderCacheSnapshot` exposes `forced_pending`
  and `bypass_prerendered`, `mark()` returns a `MarkOutcome` instead of a boolean, and the server
  logs one high-water warning at 75% of the bounded claim set before it fails closed.
- `_react` and `_typescript` in `ProjectConfig` are documented as accepted but unused. They are
  deprecated, remain deserializable so existing configs keep loading, and must not be wired to new
  behavior.

### Performance

- The incremental bundler cache no longer stats files for mtime and size. Freshness is decided from
  the source text's own length, which removes a filesystem metadata read per module and keeps the
  recorded size consistent with the check that uses it. `compute_dirty_set` and its transitive
  dependency tracking are gone.
- Decorator stripping reuses the `ModuleAst` the compile phase already parsed through the new
  `transform_with_plan()` and `strip_decorators_with_plan()`, eliminating a redundant parsing walk.
  `transform_with_options()` remains as a wrapper.

### Toolchain

- Node.js 22.13.0 is now the minimum, updated across CI workflows, README badges, and the plugin
  package template.
- Release workflows gained OIDC permissions, npm publish order moved into a validation script, and
  registry propagation handling was made more resilient.

## v1.0.29 (2026-08-10)

### Breaking: shortened adapter factory exports

The named factory export from every first-party deployment adapter no longer ends in `Adapter`.
Update imports and calls as follows; the default export changed to the same new factory name. Option
types and generated deployment artifacts are otherwise unchanged.

| Package                      | Before 1.0.29       | In 1.0.29                       |
| ---------------------------- | ------------------- | ------------------------------- |
| `@ruvyxa/adapter-aws`        | `awsAdapter`        | `aws`                           |
| `@ruvyxa/adapter-bun`        | `bunAdapter`        | `bun`                           |
| `@ruvyxa/adapter-cloudflare` | `cloudflareAdapter` | `cloudflare`                    |
| `@ruvyxa/adapter-deno`       | New package         | `deno`                          |
| `@ruvyxa/adapter-firebase`   | `firebaseAdapter`   | `firebase`                      |
| `@ruvyxa/adapter-netlify`    | `netlifyAdapter`    | `netlify`                       |
| `@ruvyxa/adapter-node`       | `nodeAdapter`       | `node`                          |
| `@ruvyxa/adapter-railway`    | `railwayAdapter`    | `railway`                       |
| `@ruvyxa/adapter-render`     | `renderAdapter`     | `render`                        |
| `@ruvyxa/adapter-static`     | `staticAdapter`     | `static` (import with an alias) |
| `@ruvyxa/adapter-vercel`     | `vercelAdapter`     | `vercel`                        |

For example, replace `import { nodeAdapter } from '@ruvyxa/adapter-node'` with
`import { node } from '@ruvyxa/adapter-node'`, then use `adapter: node()`. Because `static` is
reserved in direct function declarations, import it as an alias such as `staticOutput`.

### Breaking: image optimization is opt-in

- **Responsive variants are no longer generated automatically.** `variantWidths` is unset by
  default, so a build publishes one WebP per source instead of a full responsive set; applications
  that want the previous behavior set `variantWidths` (or use on-demand image optimization). The
  `defaultVariantWidths` presets are gone.
- **`keepOriginal` now defaults to `false`.** Original images are no longer published unless
  explicitly re-enabled, shrinking build output and deployment size. Build warnings now say what
  each setting causes: a raw `<img>` referencing a missing original when `keepOriginal` is off, and
  suboptimal WebP usage when it is on.

### MDX and Markdown compilation

- **Markdown and MDX compile through the configured `@mdx-js/mdx` pipeline instead of the native
  fallback.** A persistent JavaScript content host compiles each document once per unique source,
  and the compiled content is reused by dependency scanning and code generation. Raw HTML is escaped
  in `.md` documents, heading exports are collected, and each document is wrapped in a stable
  `ruvyxa-content` article container.
- **Added the `compile_content` build-plugin hook.** A plugin can compile (or rewrite) `.md`/`.mdx`
  sources itself; `config.markdown` — `gfm` (on by default), remark/rehype plugin arrays, and
  `remarkRehypeOptions` — flows into the host. Content-cache keys include the markdown configuration
  fingerprint, so changed plugins or options cannot serve stale compiled output.
- **MDX component providers are discovered automatically.** `mdx-components` files are located by
  walking ancestor directories (bounded by the project root), covering `.tsx`, `.ts`, `.mts`,
  `.mjs`, and the classic `.js`/`.jsx` forms, and the discovered provider is imported into each MDX
  document that can reach it. Provider paths participate in the content cache key, and the
  `providerImport` option can inject a provider explicitly.

### Tooling and CI

- **Raised the minimum Node.js version to 22.13.0** across the CLI, packages, templates, examples,
  documentation, and the CI matrix (12.22 → 13.22 in one step; prior releases required 22.12.0).
- Pinned pnpm to 10.34.5 after a brief 11.21.0 excursion that the minimum-runtime test matrix
  rejected, and added workspace assertions that verify the Node and pnpm versions running tests
  match the documented minimums.
- Added a scheduled security audit workflow: a RustSec pass over `Cargo.lock` production
  dependencies and an `pnpm audit` pass over production packages, both also triggered whenever a
  dependency manifest changes. The CI test matrix now spells out per-platform Node versions.

### Deno runtime and deployment

- Added Deno as a JavaScript runtime for configuration, rendering, API routes, actions, adapters,
  and build plugins. Select it with `runtime: 'deno'`, `RUVYXA_RUNTIME=deno`, or `--runtime deno`.
  With no explicit selection, runtime detection falls back from Node to Bun to Deno.
- Added `@ruvyxa/adapter-deno` for a self-contained Deno deployment. It emits `deploy/deno/server`,
  optional static output, and a standalone command: `deno run -A --no-prompt server/index.mjs` from
  the copied deploy directory.
- Added Deno package-manager detection for `deno.lock`, `deno.json`, and `deno.jsonc`, including
  Deno task guidance in created projects. Deno is run with the permissions trusted local project
  configuration and plugins require; do not use it to execute untrusted project code.

### Worker admission and route matching

- Added `RUVYXA_WORKER_MAX_QUEUE`, which bounds waiting render work to four requests per configured
  active worker slot by default. A full queue returns `RUV1705` instead of retaining request
  payloads without limit; `ping` and invalidation requests stay outside the render queue.
- Static routes are now indexed for direct lookup while parameterized routes preserve their existing
  specificity order. The serverless handler shares the canonical-input matcher, so this performance
  improvement keeps the existing validation and route-precedence semantics.

### Documentation

- Updated the English and Thai tutorial trees with learning goals and checkpoints, clarified that
  Ruvyxa is a web framework rather than a React-only framework description, and documented the new
  runtime, adapter, queue-control, image, and MDX behavior. ARCHITECTURE.md gained system-boundary,
  repository-topology, and state/failure/compatibility sections plus a Deno-aware system diagram,
  and the README benchmark tables were refreshed for 1.0.28.

## v1.0.28 (2026-08-07)

### Breaking

- **`@ruvyxa/react` no longer re-exports the route-matching engine.** `compilePattern`,
  `routeSpecificity`, `compareSpecificity`, `normalizeMatchPath`, and `createRouteMatcher`, plus the
  `RouteMatch` and `RouteManifestEntry` types, now live only in the new `@ruvyxa/core/route-match`
  entry point. `@ruvyxa/react` still exports the `RouteParams` type, which is what `useParams()`
  returns and the only one of these an application normally touches. Update any import of the others
  to `@ruvyxa/core/route-match`. These were engine internals sitting in a user-facing package, and
  exporting them there is what allowed the duplicate ports described below to accumulate unnoticed.

### Route Matching Correctness

- **Removed the third independent implementation of route matching.** Resolving a URL to a route was
  ported three times: the Rust router used by `dev` and `start`, the client router in
  `@ruvyxa/react`, and a private copy inside `runtime/serverless-handler.mjs`. Nothing kept them in
  agreement except review, and a URL that resolves differently between them renders a different page
  on a soft navigation than on a reload — a defect that only appears after deployment. The
  JavaScript hosts now share one module, `@ruvyxa/core/src/route-match.ts`; the handler receives it
  as `runtime/route-match.mjs`, a committed copy of that module's compiled output which
  `adapter-runner.mjs` places in every function bundle alongside the handler, so a deployed function
  still resolves no bare specifiers. The copy is committed rather than generated on demand because
  the Rust test suite executes the adapter runner before any JavaScript build has run, and because
  the file ships in the package's `files` — a generated-only file would be absent in both cases.
  `ruvyxa`'s build runs `scripts/sync-route-match.mjs --check`, so editing the shared module without
  regenerating the copy fails the build with the command to fix it instead of shipping two matchers
  that disagree.
- **Added a cross-language conformance suite.** The Rust router cannot share the JavaScript module,
  so `tests/fixtures/route-match-conformance.json` pins canonicalization and match results for both.
  It is replayed by `crates/ruvyxa_dev_server/src/router.rs` and by
  `packages/@ruvyxa/react/test/route-match.test.mjs`, which also drives the serverless handler's own
  dispatch path. A behaviour change made in one language and not the other now fails a test.

### On-demand Revalidation

- **Added `revalidatePath()`, callable from an API route or a server action.** It takes a concrete
  URL (`/blog/hello`), not a route pattern, and rejects anything else with a message naming the
  mistake. The invalidation is queued onto the calling request's response, so a client that follows
  a successful action with a navigation cannot arrive before the cached document has been cleared.
  Every render strategy is covered: for SSR and CSR the cached document is dropped; for SSG, ISR,
  and PPR the next request additionally bypasses the HTML the build wrote to disk — the copy that
  would otherwise keep being served regardless of cache state. There is deliberately no
  `revalidateTag()`: in Next.js a tag labels a fetch-cache entry, and Ruvyxa has no fetch cache for
  one to label. Supporting tags would mean inventing a page-level tag declaration and a tag-to-route
  index, which is a design decision rather than an addition to this API.
- Pending revalidations are tracked separately from the cache's own LRU lifecycle and are handed to
  exactly one renderer, so two requests arriving together cannot rebuild the same page twice. The
  serverless handler and the worker runtime pass revalidations through to the function instance the
  response returns from.
- **An application can now provide `instrumentation.ts`, whose `register()` runs once per server
  process before the first request.** That is the process a render runs in: the worker under
  `ruvyxa dev` and `ruvyxa start`, and the function instance after a deploy. It is where an
  OpenTelemetry SDK, an error reporter, or a metrics exporter is installed, and revalidation events
  are observable through the same host. Note that stdout in that process is the worker's NDJSON
  response channel, so hooks must write to stderr.
- The demo gained `/api/revalidate` — a minimal webhook that validates a path and revalidates it —
  plus a page that exercises fresh renders. The English and Thai data-actions, UI/navigation, and
  observability guides document `revalidatePath()` and the instrumentation entry point, and both
  configuration guides add the `typedRoutes` option.

### Typed Routes

- **`<Link href>` and the imperative router are now checked against the routes the project actually
  has.** Setting `typedRoutes: true` in `ruvyxa.config.ts` (default `false`) makes `ruvyxa dev`,
  `build`, and `check` write `.ruvyxa/types/routes.d.ts` — generated before each run's validation so
  the editor is never behind — with one key per discovered route pattern. The file augments
  `RuvyxaRouteRegistry` on `@ruvyxa/react/routes`; the extension point must be that subpath rather
  than `@ruvyxa/react`, because re-exported interfaces do not take part in declaration merging.
  `RouteHref`, the type `Link`'s `href` now takes, is the union of the URLs each pattern actually
  serves: `[slug]` and `[...rest]` expand to `${string}`, `[[...rest]]` adds the parent path (a
  trailing slash is dropped so the optional segment's root matches), plus `?query` and `#hash`
  variants. External URLs stay legal — `<Link>` renders a real anchor, so any scheme, a `mailto:`,
  an in-page anchor, and a `//host` are all accepted. A URL computed at runtime is marked with
  `route(url)`, which narrows a plain `string` to `RouteHref`.
- **Opt-in and strictly additive.** Until the file is generated and the project's tsconfig includes
  `.ruvyxa/types/**/*.d.ts`, the registry is empty and `RouteHref` collapses to `string`, so every
  project that never opts in — and every project that predates the feature — type-checks exactly as
  it did before. The minimal template ships both the generated-types `include` and a
  `typedRoutes: true` setting with an explanatory comment.
- **Added `<Script>`, a third-party script component with a loading strategy.** `beforeInteractive`
  emits a real `<script>` into the server HTML — the only strategy that works on a page with
  `export const hydrate = false`, which ships no client runtime for an effect to run in;
  `afterInteractive` and `lazyOnload` defer execution, and an external URL is fetched once per page
  no matter how many times it is rendered. `resetInjectedScripts()` resets the once-per-page table
  for tests.

### Process Lifecycle

- **Fixed builds hanging forever on an unresponsive TypeScript build plugin.** The build-side plugin
  host read each hook response with a blocking `read_line` and no timeout, so a plugin with an
  unresolved promise or a blocking loop stalled the whole build with no diagnostic and no recovery
  but killing the CLI — the exact failure the module's own documentation promised not to have. Hook
  responses are now read on a dedicated thread with the same 30-second budget the middleware plugin
  host already enforced; on expiry the worker is killed, the build fails with `RUV1701`, and the
  dead worker refuses further hooks rather than pairing a late response with the next request.
- **Bounded every synchronous child process.** Config loading, adapter build and inspect hooks,
  `tsc --noEmit`, the Tailwind CLI, the one-shot page and API renderers, runtime version probes, and
  the port-conflict diagnostics all called `Command::output()`, which waits forever. A child that
  keeps its event loop alive — a config importing a module that opens a database handle, a watcher,
  a server — hung the CLI before it printed anything, and because `std::process::Child` does not
  terminate on drop, an interrupted CLI left the child orphaned. All of them now run through
  `ruvyxa_dev_server::process::output_with_timeout`, which drains both pipes on their own threads,
  closes stdin, and kills and reaps the child on every path out.

### Build Performance

- **Cached the rendered project config.** Loading `ruvyxa.config.ts` started a JavaScript runtime
  and recompiled the config bundle on every single CLI invocation, including commands that barely
  read the config. The result is now cached and replayed while its inputs hold, cutting a light
  command such as `ruvyxa routes` on `examples/demo` from ~517ms to ~167ms. The cache key is exact
  rather than approximate: the renderer reports the transitive project modules and package manifests
  that fed the dependency hash, plus — via a recording proxy over `process.env` — every environment
  variable the config actually read, so a config that branches on `NODE_ENV` re-renders when
  `NODE_ENV` changes and a config that reads nothing is pinned to nothing. The runtime and the
  renderer's own content hash are part of the key too.

### Build Resource Use

- **Build concurrency is now bounded by free memory, not by core count alone.** Route bundling and
  prerendering sized themselves from available cores, which reads as "use the machine" but ignores
  what actually runs out: each concurrent bundle holds its own parser arenas and module graph, and
  each prerender worker is a whole JavaScript runtime process. Measured on `examples/demo`, going
  from one worker to sixteen cost about 100MB of peak resident memory for a 1.4x speedup; the same
  rule on a memory-capped CI container asks for far more and is killed rather than slowed. Both
  budgets now take the smaller of the CPU budget and what free memory can hold. When free memory
  cannot be determined the previous core-based behaviour is used unchanged.
- **Starter templates no longer pin `build.workers`.** Every template — and the demo — shipped
  `workers: 4`, so every scaffolded project was capped at four bundling workers regardless of the
  machine it built on. The field is now unset, which selects the machine-aware default. An explicit
  value still lowers the CPU budget but no longer escapes the memory bound.

### Bundler and Linker Correctness

- **Fixed tree-shaking dropping live exports that share a line with an unused one.** The linker
  emits one line per source `export` statement, so a barrel's `export { a, b, c } from "./mod"`
  becomes three `__exports.… = …;` assignments on a single line. The shaking pass read only the
  first name on the line and commented out the whole line when that name was unused, which emptied
  pure re-export barrels such as `@ruvyxa/react`'s `dist/index.js`. Consumers then hydrated with
  `undefined` for every imported component (React error #130). Each assignment is now judged on its
  own; lines that are not made up entirely of simple export assignments are left untouched.
- **Fixed tree-shaking dropping exports that are only reached through a namespace alias.** The
  linker binds `import * as ns from "./mod"` to `const ns = __ruv_xxx__;`, so a later `ns.member`
  read never appears as `__ruv_xxx__.member` and the pass concluded every export of that module was
  dead. A module whose namespace is read as a whole — by a namespace import, or by the
  default-import interop expression for CommonJS packages — now keeps all of its exports.
- **Fixed `ruvyxa check`/`analyze` rejecting a client component that `ruvyxa build` compiled without
  complaint.** `ruvyxa_graph` carried its own copy of the private-environment-variable filter for
  the RUV1008 diagnostic, separate from the one `ruvyxa build` enforces, and the copy had silently
  lost the `NODE_ENV` exemption. A client component containing the single most common line in React
  — `process.env.NODE_ENV !== 'production'` — built cleanly and failed `check`. The rule is now one
  function, `ruvyxa_bundler::boundary::env_read_is_private`, read by both.
- **Added full JSONC support for `tsconfig.json`/`jsconfig.json`.** Only `//` line comments were
  stripped; `/* */` block comments and trailing commas — both valid JSONC, and both what
  `tsc --init` generates, since every option it writes is documented in a `/* */` block — made
  parsing fail, and a failed parse silently contributed no `baseUrl`/`paths`. Every aliased import
  in a project whose tsconfig used block comments failed to resolve, with the reported error naming
  the import rather than the config that had been skipped.
- **A malformed tsconfig is now reported instead of silently ignored.** `ruvyxa doctor` showed
  `tsconfig.json  exists` whether or not the file could actually be parsed. It now reports the parse
  error by name, and a broken `tsconfig.json` no longer blocks a valid `jsconfig.json` sitting
  beside it from loading — each candidate is tried in turn.

### Dev Server Correctness

- **Fixed CSR pages never being invalidated in the dev server's render cache.** The prefix list
  `invalidate_route` strips before matching a cache key against a changed route covered
  `ssg:`/`isr:`/`ppr:` but not `csr:`, so a CSR page's cached render was never found by file-change
  invalidation and kept serving a stale version of an edited file until its entry's TTL (5 minutes
  in dev) expired. Key construction and the prefix list are now the single function and constant
  (`page_cache_key`, `RENDER_NAMESPACES`) that both sides read.
- **Fixed a weak ETag never matching itself on revalidation.** The `If-None-Match` comparison
  stripped a candidate value's `W/` prefix but not the locally-computed target's, so a client
  holding a weak validator — now produced for every streamed large asset, see Performance below —
  always missed and re-received the full body instead of a `304`.
- Client bundle requests (`/__ruvyxa/client/<hash>.js`) now answer a revalidation from the same
  fingerprint cache `public/` files already used, instead of re-reading and blake3-hashing the whole
  bundle to produce an empty `304` response.
- **Fixed the Rust and JavaScript servers disagreeing about a `public/` file's Content-Type.** The
  two tables were written independently: `.wasm` fell back to `application/octet-stream` in Rust,
  which makes `WebAssembly.instantiateStreaming` refuse the module outright; `.woff`, `.woff2`,
  `.gif`, `.ico`, `.map`, and `.html` fell back the same way; `.webmanifest` fell back in the
  JavaScript table instead. Separately, the list of extensions routing recognizes as a static asset
  and the list with a Content-Type for one had different membership — `.webm`, `.mp4`, `.mp3`,
  `.ogg`, `.wav`, `.mov`, `.ttf`, `.otf`, `.eot`, `.bmp`, and `.apng` were routed as assets and then
  served as an opaque download, which stops a `<video>` from playing and makes a browser download a
  font instead of using it. Both tables and both lists are now pinned to
  `tests/fixtures/static-asset-conformance.json`, replayed by a Rust test and a JavaScript test.
- **Fixed the default security-header list being maintained as two hand-written copies inside one
  file** — one that adds the seven headers, one that removes them when `security.headers: false`. A
  header added to one copy and not the other meant disabling security could silently keep sending a
  header the project had asked to turn off. Both directions now read one list
  (`DEFAULT_SECURITY_HEADERS`), pinned against the equivalent JavaScript table — which cannot share
  the Rust code — by `tests/fixtures/security-headers-conformance.json`.

### Reliability

- **Consolidated four independently-written atomic file writers into one.** The bundler's compile
  cache, its incremental graph manifest, the CLI's client-artifact cache, and the image optimizer's
  cache each wrote their own "temp file, then rename" sequence, and had drifted in the way a copy
  drifts: two derived a temporary's name only from the target path, so two writers publishing the
  same cache entry could race on one temporary file; one recovered from a failed rename by reading
  the temporary back with `unwrap_or_default()`, so a recovery that itself failed replaced a good
  cache entry with zero bytes; one leaked its temporary file whenever the first write failed,
  leaving `.tmp` files behind on every attempt under a full disk. All four now publish through
  `ruvyxa_bundler::atomic_file::write_atomic`.

### Performance

- **Cached pages now serve a pre-compressed copy on every hit after the first.** A cache hit
  previously still paid a full brotli/gzip pass through the outer compression layer for identical
  bytes on every single request. Render-cache entries now carry a compressed copy built lazily
  alongside the HTML — built once, on the first request that can use it, and shared by every later
  hit, including concurrent ones. Documents under 256 bytes are left uncompressed (the header
  overhead usually outweighs the saving), and every cached response now carries
  `Vary: Accept-Encoding`.
- **Large public assets are streamed instead of being read into memory before the first byte is
  written.** A file above 8 MiB (`RUVYXA_STREAM_ASSET_THRESHOLD_BYTES`) is now sent to the response
  as a stream; previously, peak server memory scaled with the number of large files being served
  concurrently, so a handful of clients downloading a large video was enough to exhaust it. A
  streamed asset's ETag is weak (size + modification time), since a content hash cannot be produced
  without holding the whole file in memory at once.
- **Bounded how large one NDJSON line from a Node/Bun worker's stdout or stderr can grow before
  being read.** `AsyncBufReadExt::lines()` accumulates without limit until it finds a newline, so a
  worker emitting one very large or corrupted line was buffered in full on the Rust side before
  anything could reject it — the failure mode was the whole server process running out of memory,
  with nothing naming the worker that caused it. Defaults to 64 MiB, configurable with
  `RUVYXA_WORKER_MAX_LINE_BYTES`; over the limit, the pool replaces the worker instead of trying to
  resynchronize a framing it can no longer trust.

### Templates and Examples

- Continued building out the Ruvyxa runner game added in 1.0.27: a pause mechanic and a four-frame
  gait animation, an expanded obstacle and boss sprite library with animation frames,
  boss-difficulty balancing and visual-clarity passes, an autopilot AI (Alt+T) that plans by
  simulating the runner's actual physics and hitboxes — including an exact early-exit over its
  delayed-action search — rather than following fixed timings, and win conditions with adaptive boss
  scaling.
- Fixed a projectile able to score a hit against both an obstacle and the boss standing behind it in
  the same frame: "this shot is spent" was represented only by moving it off-screen, which the
  remaining collision checks in that frame never re-read.
- Fixed a single death able to run the end-of-game logic once per overlapping hazard instead of once
  per death — one collision against multiple hazards fired the death particle burst several times
  and, more importantly, could jump the autopilot's caution level up by more than one step per
  death, defeating its gradual difficulty-adaptation design.

### create-ruvyxa CLI

- **Scaffolding is now interactive on a real terminal.** When no template or project name is given
  and both stdin and stdout are TTYs, `create-ruvyxa` prompts for a project name (line editing, with
  a default accepted on Enter) and lets the template be chosen through an arrow-key menu — `j`/`k`
  also work, vim-style — with a one-line description per starter. Terminal state and cursor
  visibility are restored when the prompts finish.
- **A branded startup banner** draws the Ruvyxa mascot in the same One Dark palette the rest of the
  output now uses, and scaffolding runs under an animated braille spinner. The mascot spinner runs
  for a minimum of one full loop before it can stop, its stop is awaited so the interval is cleared
  in order, and the completion message is no longer dropped when output is piped or redirected.
  Next-steps print with syntax-highlighted commands.
- **The project summary is now the real file tree.** The hardcoded six-line summary is gone;
  `createRuvyxaApp` returns the files it actually wrote, and the scaffolder renders them nested,
  directories first, capped at 24 entries with an overflow line. Entries are coloured by role —
  directories, markup, modules, styles, config, assets, docs, and dotfiles each get their own hue
  from a One Dark palette that emits truecolor when `COLORTERM` advertises it and falls back to the
  nearest xterm-256 slot otherwise. Help text and the missing-template error source from the same
  `STARTER_TEMPLATES` map as the menu.
- **Terminal redrawing is now frame-aware.** A `createFrame` utility owns cursor position and screen
  updates; relative cursor movement replaces the previous DECSC/DECRU save/restore, which drew stale
  content once the terminal scrolled. `tty.ts` provides `visibleWidth`, `stripAnsi`, and
  `physicalRows` so wrapping is measured as the terminal sees it, and a `canRedraw` check falls back
  to plain sequential output when in-place drawing is not possible.

### Plugin Scaffolding

- **Fixed `ruvyxa plugin create` generating a test that fails for every plugin name except one.**
  The scaffolded `test/plugin.test.mjs` asserted the plugin's name twice: once against the
  `__PLUGIN_NAME__` placeholder and once against the literal `request-logger` the template was
  authored with. Placeholder substitution cannot rewrite a plain literal, so the stray assertion
  survived into every generated plugin and failed at `npm test` — step 3 of the "next steps" the
  command prints — and, because the generated `package.json` runs `prepublishOnly: npm test`,
  blocked publishing too. The duplicate assertion is removed. Scaffold tests previously all used
  `request-logger`, which made a hardcoded literal indistinguishable from a substituted placeholder;
  a new test now scaffolds under an unrelated name and rejects any residual authoring literal or
  unsubstituted placeholder across every template file.

### Internal

- New `ruvyxa_dev_server::response` module: response construction and the shared security-header
  table, extracted out of `lib.rs`.
- New `ruvyxa_bundler::atomic_file` module: the durable-write primitive behind the Reliability fix
  above.
- Added `scripts/check-template-mirrors.mjs`, wired into `pnpm release:validate`, keeping
  `templates/minimal/app/components/ruvyxa-runner.tsx` and its `examples/demo` copy byte-identical —
  five commits had edited both by hand, and the projectile/end-of-game defects above lived in both
  copies as a result.
- Declared `brotli` and `flate2` (already compiled into the build through `tower-http`'s compression
  features) and `tokio-util` (for the streaming response body) as direct `ruvyxa_dev_server`
  dependencies, adding no new crate to the build.
- Documented the bundler's custom tree-shaking pass (Pass 0) in `ARCHITECTURE.md`, including the
  per-assignment and opacity rules that carry its correctness.
- Corrected the linker's module docs: named imports bind per-member (`const a = __ruv_xxx__.a`), not
  by destructuring the namespace. The stale form mattered because tree-shaking's opacity rule turns
  on exactly which import forms read a namespace as a whole.
- Removed an unreachable branch in the tree-shaking pass that tested for `return __exports;`, a line
  the linker never emits (it emits `return module.exports;`) and which could not match a trimmed
  line anyway.
- Dropped the unused `chrono` dependency from `ruvyxa_dev_server`.
- Added a request-context runtime: the Rust dev server, worker pool, adapter runner, and serverless
  handler carry the route pattern, request headers, queued `setCookies`, and revalidation state
  through a shared `request-context` module, installed on `@ruvyxa/core/server` via
  `installRequestContextHost()` so the `node:async_hooks` built-in never reaches an edge or browser
  bundle. `revalidatePath()`, `cookies()`, `headers()`, and `draftMode()` all read from it.
- `ruvyxa_tui`'s column-alignment test now strips ANSI escape sequences before measuring text width,
  so its field-and-phase-line assertion holds on an interactive terminal with colour enabled instead
  of only in CI where colour is off.

## v1.0.27 (2026-08-05)

### Breaking changes

- Renamed the scaffold command from `ruvyxa add` to `ruvyxa adds`. Generated applications now use
  `npm run adds -- form` (or `data-table` / `auth`).

### Terminal UI

- Added the `ruvyxa_tui` crate with a spinner, layout, mascot, progress, and theme module, and wired
  it into the CLI and dev server. Build phases now render as animated spinners with progress
  tracking instead of static printed lines.
- Separated the terminal's two streams: progress bars and spinners write to stderr, results to
  stdout. `ruvyxa build > log` now captures a clean result log without animation bytes, animation is
  disabled when either stream is captured, and color is preserved on stdout.
- Standardized CLI output across commands: semantic color functions (`info`, `note`, `number`), a
  shared `print_success_banner` with elapsed time, and reusable column-width helpers. `doctor` now
  reports the installed Ruvyxa packages and their version compatibility beside the CLI version.
- Replaced byte-count width math with character-based `display_width`, so multi-byte route names
  (Thai, Arabic, …) no longer break table alignment, and fixed Windows path joining to concatenate
  component by component instead of with a literal slash.

### Bundler and Linker Correctness

- **Added CommonJS-to-ESM default-import interoperability.** Compiled ES modules carry a
  `__esModule` marker, and `interop_default()` binds default imports to `module.exports` for
  CommonJS packages or the `default` export for ESM, for both plain and re-exported default imports.
  The prerender and dev-server pipelines apply the same rule.
- **Added text-span tracking.** `ModuleAst` records byte ranges for strings, comments, regexes, and
  template-literal text, and `is_code_offset()` lets the linker skip rewriting inside text. The
  `real_imports` set and `static_import_specifiers()` are gone; import-like content in documentation
  strings is no longer rewritten.
- **Added unresolvable import detection and deferred failure stubs.** External imports carry target
  and importer labels, bare specifiers no longer leak into browser bundles, and unresolvable imports
  defer failure in a way that survives minification with `RUV1610`/`RUV1611` file context intact.
- Client bundles now replace unresolved `require()` calls with a runtime `RUV1610` error under the
  new `drop_unresolved` flag (default off for SSR/edge bundles), instead of shipping a bare
  `require` that throws `require is not defined` in the browser.

### Server-only builds

- Added `ruvyxa build --server-only` for API-only artifacts. Only the `node` and `bun` targets
  accept it; a server-only build with page routes that cannot be deployed is rejected before any
  staging directory is created. Style collection and image optimization are skipped, and the build
  summary shows a "production · server-only" profile.

### Templates and examples

- Added an interactive Ruvyxa runner game to the minimal template and the demo app: sprite
  rendering, jump/duck/shoot controls, progressive obstacles (bugs, errors, malware), a score-based
  boss encounter, particles, best-score persistence, and keyboard/touch input.

### Fixed

- Markdown and MDX pages no longer gain graph edges from imports shown inside fenced code examples.
  Every other reader masked those examples before scanning; the import-edge walk did not, so a
  documented `import './config'` pulled a real module into the page's client graph and could raise
  RUV1007, RUV1008, or RUV1010 against code the page never runs. Source masking now happens where
  the file is read, so no reader can skip it.
- Fixed a window in the dev-server render cache where an entry expiring at the same moment as a
  write of the same key could leave that key out of the recency list. The eviction path recovered by
  clearing the entire cache, so the symptom was an unexplained loss of every cached render.

### Performance

- Image resizing now uses SIMD (AVX2/SSE4.1/NEON) convolution instead of a scalar loop, through
  `fast_image_resize`. It is the same Lanczos3 filter, so output is unchanged. Producing all eight
  responsive widths for a 6000x4000 source drops from 3628 ms to 68 ms of CPU. On a build with
  twelve 4000x3000 sources, where that CPU is actually contended, the whole image stage goes from
  16.2 s to 7.6 s.
- A rebuild whose images are unchanged no longer decodes them. Every output is content-addressed, so
  the cache decides before any pixel is touched, and the manifest reads its dimensions from the file
  header (2.4 ms against 116 ms for a full decode). Twelve cached images: 350 ms to 242 ms.
- Pixels are handed to the resizer and the WebP encoder by reference. `to_rgb8()`/`to_rgba8()`
  cloned the whole image on every use — 68 MB per call on a 6000x4000 source, nine times per file.
- Each source is hashed once instead of once per output. The full-size encode and all eight variant
  encodes are now one flat job list rather than a `rayon::join` that pinned the longest job, the
  full-size encode, to one side of a binary split.
- Added `image.effort` (libwebp's `method`, 0-6, default 4). Encoding is the floor on image build
  time — libwebp cannot split a single lossy encode across threads, and `thread_level` was measured
  to make no difference. On a 6000x4000 source, effort 2 is 1.8x faster for 18% more bytes and
  effort 0 is 2.9x faster for 15% more. The default is unchanged so upgrading cannot silently
  inflate a deployed asset set.
- The runtime image endpoint shares the same resize and encode path, and its LRU cache promotes an
  entry in constant time instead of scanning its recency queue on every hit.
- Route discovery and validation now read and scan each module once per run. A page was read three
  times and scanned four; a component shared by many routes was re-read for each of them, because
  rendering-strategy detection built a throwaway edge cache per route. Diagnostics and detected
  strategies are unchanged.
- Rendering-strategy detection no longer reads and masks a page's entire reachable dependency graph
  before the rules that answer from the page's own exports. Pages that declare `"use client"`,
  `ppr`, `revalidate`, or `getStaticParams` now skip that walk entirely. A page matching one of
  those rules also keeps its declared strategy when a dependency cannot be read, instead of falling
  back to SSR.
- The auth runtime compiles its OAuth route pattern once per `createAuth()` instead of once per
  request that reaches the auth handler.
- Bundler source handling shares allocations through `Arc<str>` instead of cloning `String`s:
  `read_source()` returns an `Arc<str>`, content modules and cache paths borrow it, and
  `compile_content_module_shared()` avoids one extra string copy per content module.

### Documentation

- Removed the pinned framework version from the English and Thai documentation homes. It named the
  release the docs were written for and went stale on every bump.
- Corrected `ARCHITECTURE.md` against the code it describes. Eleven documented Rust APIs did not
  exist under any name — `RuvyxaCompiler`, `check_boundary`, `produce_iife`,
  `produce_server_module`, `WorkerPool` with crossbeam channels, `compile_all`, `rewrite_env_vars`,
  `validate_route_path`, `BundleProfile`, and a `ModuleRegistry`/`SharedCache`/`DiagnosticCollector`
  lock hierarchy for types the workspace never defined. Each is now the real signature, plus
  corrected `BundleOptions`, `BundleInput`, and `CompiledModule` structs, the real emit layout, and
  the actual lock ordering.
- Repointed the stale documentation paths in `CONTRIBUTING.md`, and dropped the `docs/` links from
  the `create-ruvyxa` README: that directory is not in the published tarball, so they resolved in
  git and 404'd on npm.
- Added `@ruvyxa/testing` to the README package table, dropped a hardcoded export count, and
  replaced the "Complete Error Catalog" claim — the linked page is a symptom table covering a
  fraction of the 60+ codes, not a catalog.

### Internal

- `pnpm release:validate` now verifies every relative Markdown link and heading anchor in the
  repository. The 1.0.26 documentation restructure left 25 dead links behind, including two on the
  `create-ruvyxa` npm package page, and nothing in the toolchain could see them. All are repointed
  at their successors under `docs/en/`.
- Removed the named-export list from the bundler's module facts. It was collected on every scan and
  read only by its own tests.
- `@ruvyxa/testing` now declares its `@ruvyxa/core` peer range as `workspace:^`, matching every
  other package, so releases cannot leave it pinned to an older minor.
- Consolidated the image pipeline into `ruvyxa_dev_server::image_codec`: `fast_image_resize` and
  `webp` are no longer direct CLI dependencies, the optimizer imports the shared module, and image
  dimensions are checked from the file header before a full decode as a second memory-exhaustion
  guard. The image manifest cache fingerprints settled outputs to reduce redundant JSON parsing.
- README updates in this release: Node.js minimum raised to `>=22.12`, the `build` command now
  documents `--adapter` and `--server-only`, and a Requirements plus Quick Start section was added.
- Architecture documentation now describes the real protocol shapes: HMR lives at `/__ruvyxa/hmr`
  with a single message and no client-to-server traffic, Server Actions use
  `?path=<route-path>&name=<action-name>` query parameters, and the seven default security headers
  are documented as opt-out-overridable application defaults.

## v1.0.26 (2026-08-03)

### Developer experience

- Added a self-contained interactive `ruvyxa analyze --html` report, `routes --json`, and a
  development-only `/__ruvyxa/devtools` dashboard for routes, LRU cache state, bundle metrics,
  Server Action timing, and uptime.
- Added atomic `ruvyxa add form|data-table|auth` scaffolds and the dependency-free `@ruvyxa/testing`
  package with loader, action, and cache mocks.

### Runtime and routing

- Added validated file-system i18n routing with locale detection, prerender expansion, automatic
  document language and hreflang output, and native/serverless parity.
- Added opt-in browser-native View Transitions, React 19 stable action API coverage, and bounded
  same-origin on-demand image optimization with Cloudflare image-transform integration.
- Carried validated built-in middleware policy into standalone, serverless, Cloudflare Workers, and
  Vercel Edge artifacts through Fetch-native CORS, rate limiting, timing, logging, and headers.
  Vercel now supports an explicit `edge: true` mode without Node.js polyfills.

### Documentation

- Added production-shaped Prisma and Drizzle ORM starters in English and Thai and documented the new
  CLI, runtime, adapter, image, routing, and testing contracts.

### Bundler Correctness

- **Fixed dependency scanning around regular expressions and template literals.** The shared source
  scanner now distinguishes a regular-expression literal from division, skips quoted content and
  comments correctly, and scans `${…}` interpolations as code. Imports, `require()` calls,
  re-exports, default-export validation, and client-boundary checks therefore remain visible after
  patterns such as `/["']/` and inside real template expressions.
- **Fixed interpolation scans reading into surrounding template text.** Scanner helpers are now
  bounded to the interpolation range, so text following `${import}` or `${require}` cannot be
  interpreted as a module specifier.
- **Fixed warm builds resolving aliases differently from cold builds.** The incremental graph cache
  now persists each source-specifier-to-path alias with its dependency edges. Cache entries created
  before that field are resolved fresh rather than being reused with an empty alias map, preventing
  unresolved alias specifiers in warm client bundles.

### Realtime

- **Coalesced subscription-driven reconnects.** A burst of channel subscriptions now settles into
  one queued refresh using the final channel set, instead of repeatedly opening and discarding
  sockets as each subscription is registered.

### Build Architecture

- **Consolidated source facts in the bundler AST.** The compiler, linker, boundary validation, and
  route graph now share the parsed import/export/default-export/environment-read facts. Compiled
  modules keep that parse result for the duration of a build, avoiding repeated scans while keeping
  route validation and bundling aligned.
- **Split the CLI implementation by responsibility.** Command dispatch remains in `main.rs`; build,
  caching, client bundles, prerendering, configuration, plugin bridging, diagnostics, and UI now
  live in dedicated CLI modules. This is an internal refactor and does not add or remove CLI
  commands.

### Documentation

- Updated the bundler, graph, and CLI architecture references to describe the shared scanner, cache
  and resolver behavior, and the current CLI module layout.

## v1.0.25 (2026-07-30)

### Route Metadata

- **Added `export const meta`.** A page or layout can declare document metadata; the framework
  merges every `meta` on the route root-layout-first and renders the result into `<head>`. Fields:
  `title`, `titleTemplate`, `description`, `canonical`, `robots`/`noindex`, `lang`, `alternates`,
  `image`, `imageAlt`, `siteName`, `type`, `locale`, and `card`. `meta` may be an object or a
  synchronous function of `{ path, params }`.
- A level's own `title` is never formatted by its own `titleTemplate`, so a layout template formats
  its pages without reformatting the layout's own title.
- `lang` is applied to the `<html>` element of the document each server render produces, covering
  SSR, SSG, PPR, prerender, and serverless. Client-side navigation does not change it.
- Metadata is composed as a sibling of the route's layouts, so a suspended layout cannot hold the
  document title back past the flushed shell, and no wrapper element is created per render.
- Added `Meta`, `MetaFactory`, `MetaExport`, `MetaContext`, and `MetaAlternate` types to
  `@ruvyxa/react`.

### Crawler Discovery Files

- **`ruvyxa build` now generates `robots.txt` and `sitemap.xml`** from the route manifest and the
  URLs the build prerendered, instead of leaving both to opt-in plugins. A file of the same name in
  `public/` always wins.
- Added the `site` configuration block: `url`, `sitemap`, and `robots`. When `url` is absent the
  build resolves a production-only origin from `RUVYXA_SITE_URL`, Vercel, or Netlify. Structured
  options now support sitemap exclusions/additional paths and Next-style robots rule groups.
- Sitemap output now validates and escapes absolute URLs and automatically shards at the protocol's
  50,000 URL or 50 MB limits. Exact application routes can own `/sitemap.xml` or `/robots.txt`
  without being shadowed in production, and both Rust and standalone servers return the correct
  UTF-8 XML/plain-text content types.
- Added Next-style rich sitemap entries through `site.sitemap.defaults` and `site.sitemap.entries`:
  modification dates, change frequencies, priorities, language alternates, images, and videos. Core
  and first-party plugin output use readable multi-line XML, conditional namespaces, strict
  URL/date/value validation, and the same sharding limits.
- **Fixed `/robots.txt` and `/sitemap.xml` being answered with an HTML page.** Those exact paths now
  return 404 when no file backs them, rather than letting a bare dynamic route such as `/[lang]`
  capture them. `dev`, `start`, and the serverless handler apply the same rule.

### Plugins

- **Added the `head` declaration.** A plugin contributes `link`, `meta`, `noscript`, `script`, and
  `style` elements to every rendered document's `<head>`, declared once at config load and injected
  by the server with no per-request round trip into the plugin host. Attribute values are escaped
  and the element list is closed, so a declaration cannot end the head early.
- **Added `createPluginHarness()`**, exported from `ruvyxa/plugin-harness`. It runs `register(api)`
  against recording sockets and exposes the request, response, route, build, dev, diagnostics, and
  head entry points the server uses, so a plugin can be tested without booting an application.
- **Added the `fonts()` built-in plugin.** It downloads Google Fonts stylesheets and their `.woff2`
  files at build time, rewrites the URLs to local paths, and declares the self-hosted stylesheet in
  `<head>`, removing a render-blocking third-party origin from the critical path. A network failure
  reports a diagnostic instead of failing the build.
- `definePlugin` validation errors now carry the `RUV2102` diagnostic code instead of raising bare
  `TypeError` messages.

### Security

- **Fixed every server action being rejected with `403 Cross-origin action request blocked` behind a
  TLS-terminating proxy.** The same-origin check compared the request's scheme against a hardcoded
  `http` whenever no trusted proxy reported one, so an `https` origin never matched — and the
  comparison was inverted relative to its intent, admitting a plain-`http` origin while blocking the
  secure one. The host comparison, which is the check that actually stops CSRF, now stands on its
  own; the scheme is compared only when a trusted peer states it through `X-Forwarded-Proto`.
  Deployments whose proxy is neither loopback nor listed in `security.trustedProxyIps` — the
  ordinary Docker Compose, Kubernetes, and managed-platform-edge shapes — work without
  configuration. Setting `trustedProxyIps` remains recommended: it is what enables forwarded
  client-IP detection and restores the strict scheme comparison.
- **`security.trustedProxyIps` accepts CIDR ranges.** Entries are matched as prefixes (`10.0.0.0/8`,
  `2001:db8::/32`), a bare address means a host route, and an IPv4 range also matches the
  IPv4-mapped form (`::ffff:10.0.0.9`) a dual-stack listener reports. Previously only exact
  addresses worked, which made trusting a proxy pool impractical. An unparseable entry now fails
  startup with `RUV1602` instead of being silently discarded, so a typo can no longer leave a proxy
  untrusted and every client sharing one rate-limit bucket.
- **Fixed the action rate limiter being usable to lock out every other client.** It tracked a map of
  live keys capped at 10,000 entries and denied any key it could not admit, so filling the map —
  trivial by rotating source addresses within an IPv6 `/64` — denied service to every first-time
  client until the window elapsed. Counters now live in a fixed 8,192-slot array with per-process
  hash seeding: memory no longer depends on how many clients have been seen, admission is never
  refused for lack of room, and a slot collision can only limit a client early, never grant it extra
  budget.
- **`@ruvyxa/auth` now rate-limits per client in addition to per identity.** The existing bucket
  keys on the email, so one source could try `rateLimit.max` passwords against an unlimited number
  of accounts — the shape of credential stuffing and account enumeration. A second bucket keyed on
  the client alone, with five times the budget, caps that total. The larger budget keeps shared
  egress (offices, mobile carriers, CGNAT) working. **This can return `RUV3102` where a request
  previously succeeded**, for traffic that authenticates many distinct identities from one client
  key. Configure `clientIp` in production — the user-agent fallback is client-controlled and
  therefore rotatable.
- **A plugin hook that reached the worker is no longer retried automatically.** Any delivery failure
  was treated as "worker gone" and retried, so a `request` or `response` hook whose worker died
  after receiving the request could run its side effects twice. Write and flush failures — where the
  worker provably never saw the request — are still retried; a failure while reading the response is
  retried only for hooks with no observable effect.

### Correctness

- **Fixed a page whose default export is re-exported being reported as missing one.** Route
  validation tested for the literal text `export default`, so `export { Page as default }`,
  `export { default } from './impl'`, and `export * as default from './impl'` all failed `RUV1004`,
  while the same text inside a string or comment passed. Detection now shares the bundler's scanner
  (`ruvyxa_bundler::ast::has_default_export`), which skips strings and comments and recognises every
  valid form. `export type { X as default }` is correctly rejected, since a type export erases.
- **`ctx.path` in a client bundle is the actual pathname again.** It fell back to the route pattern,
  so a page rendered at `/blog/hello` saw `/blog/[slug]` whenever the request path global was
  absent. The pattern is now published separately as `__RUVYXA_ROUTE_PATTERN__` and `ctx.path` falls
  back to `location.pathname`. The router seeds its initial snapshot from the same global, so the
  first `useRoute()` reports the pattern the server rendered rather than a re-derived guess.
- `router.refresh()` on a route whose bundle is not registered now throws a message naming the route
  and what to do, instead of failing inside the renderer with no context.

- **Fixed the Node compiler mis-linking any module containing a regular expression with a quote.**
  The source scanner had no regex-literal handling, so a pattern such as `/("[^"]*")/` opened a
  phantom string that ran to the next quote anywhere later in the file; every `import` and `export`
  in between was read as string content and survived into the bundle, producing
  `SyntaxError: Unexpected token 'export'` at runtime.

### Performance

- **Removed a file read and a full scan of the page source from every rendered request.** Each
  render re-read `page.tsx` from disk and scanned it for a default export purely to produce a
  friendlier error. Route validation already covers that case at build time, and a genuinely missing
  export is now recognised from the loader's own message, so the check no longer costs an I/O round
  trip and a scan per request.
- **Cached HTML is no longer copied on the way out.** Render-cache entries are stored as `Arc<str>`
  and served by handing back the stored allocation, so a cache hit no longer duplicates the whole
  document per request. Compiled content modules share their allocation the same way.
- **`public/` asset links are resolved once per invalidation instead of once per render.** Every
  SSG/ISR/CSR/PPR render walked the public directory to rebuild the same `<link>` list; the result
  is now memoized alongside the other runtime caches and recomputed when they are invalidated.
- **Bounded the module graphs a build worker retains.** Production prerendering imports each path
  under a fresh module URL so page state cannot leak between paths, and Node's ESM registry never
  releases a URL — so every isolated import permanently added one more module graph, and no
  in-worker cache eviction could reclaim it. A build worker is now retired after
  `RUVYXA_PRERENDER_RECYCLE_AFTER` isolated renders (default 32, `0` disables), and only while idle
  so sibling renders are never dropped. The dev server never requests isolated imports and is
  unaffected.
- **Bounded per-worker concurrency.** A worker now admits at most `RUVYXA_WORKER_MAX_CONCURRENCY`
  requests at once (default: core count clamped to 2–8) and queues the rest. Renders are CPU-bound
  and each holds a React tree, a compiled bundle, and a response buffer, so admitting a whole burst
  exhausted the heap or thrashed the CPU into timeouts that presented as hangs. `invalidate` and
  `ping` bypass the queue, since delaying an invalidation would serve stale bundles exactly when the
  worker is busiest. `ping` now also reports `queuedRequests` and `maxConcurrentRequests`.
- A worker shutdown now writes its reason to stderr, so a pool that disappears during a build is
  diagnosable instead of silent.

- **Added a build diagnostic for images that bypass the image pipeline.** A raw `<img>` pointing at
  a public PNG/JPEG the optimizer already converted is reported with its file, line, and the bytes
  the page ships versus the generated WebP. The optimization was previously performed and silently
  unused.
- Route bundles for the browser no longer carry the `<html lang>` rewrite helper, which only a
  server entry can use.

### API Naming

- `card` replaces `twitterCard` on `<Seo>` and in route metadata. `twitterCard` still works and is
  marked deprecated; the emitted `<meta name="twitter:card">` attribute is unchanged, since that is
  the name the crawler still reads.
- Site URL resolution reads one framework-owned `RUVYXA_SITE_URL` variable rather than a list of
  host-specific environment variable names.
- `ServerConfig.trusted_proxy_ips: Vec<IpAddr>` is now `trusted_proxies: TrustedProxies`, since the
  field has to hold prefixes rather than addresses. The `security.trustedProxyIps` configuration key
  and its accepted values are unchanged; only the internal Rust field, which is not published to
  crates.io, is affected. The workspace crates are now marked `publish = false` to keep that so.

### Documentation

- Added page-metadata sections to the English and Thai routing guides, the `site` block to both
  configuration guides, and `head` plus `createPluginHarness` coverage to both plugin guides,
  including a first-party plugin list that calls out `fonts()`.
- Documented the `RUV2102` plugin-definition diagnostic.
- Documented the same-origin algorithm, the sliding-window rate limiter, CIDR support in
  `trustedProxyIps`, the two `@ruvyxa/auth` rate-limit buckets, and the worker environment variables
  in both the English and Thai guides, and corrected `RUV3102`, which was documented as a WebAuthn
  failure rather than a rate-limit rejection.
- Corrected `RUVYXA_WORKER_TIMEOUT` to `RUVYXA_WORKER_TIMEOUT_MS` and the build default from 2 to 5
  minutes in the Thai API-routes guide, and rewrote the worker-pool architecture reference, which
  described an in-process Rust thread pool that no longer exists, to document the Node/Bun pool that
  does.

### Benchmarks

- Refreshed the minimal-starter benchmark on Windows with Node.js 22.23.1, npm 10.9.8, and pnpm
  11.17.0. Across three cold-cache runs, Ruvyxa 1.0.25 recorded a 1.698 s median production build,
  1.103 s dev readiness, 0.917 s production readiness, and 37,381 requests/second. The comparison
  used Next.js 16.2.12 and Astro 7.1.4 under the same harness; exact conditions and limitations are
  recorded in the README.
- The benchmark uses local packed 1.0.25 artifacts for Ruvyxa and compares minimal starter output;
  it is not a universal framework performance ranking.

## v1.0.24 (2026-07-27)

### Breaking: Unified Plugin API

- Replaced the previous `definePlugin({ name, setup })` API with `definePlugin({ name, register })`
  from the new `ruvyxa/plugin` export. Existing plugins must migrate their configuration and
  imports.
- Replaced the flat setup callbacks with grouped sockets: `http` (`onRequest`, `onResponse`, and
  `route`), `build` (`onStart`, `onResolve`, `onLoad`, `onTransform`, and `onComplete`), `dev`
  (`onFileChange`), `diagnostics`, and `native`. One plugin can register across any of these
  sockets.
- Replaced middleware `routes` with `match` and request/response callback arguments with typed
  context objects. Request hooks can continue with `next(request?)`; response hooks can continue
  with `next(response?)`.
- Migrated the built-in plugins plus `@ruvyxa/auth`, `@ruvyxa/database`, and `@ruvyxa/realtime` to
  the same contract. Each official package now exposes its plugin integration through `./plugin`.
- Replaced the old scaffolding command with `ruvyxa plugin create <name>`. The generated package is
  a TypeScript npm package with source, tests, typed framework dependencies, and a minimal headers
  example; it does not require plugin-specific package metadata.

### Plugin Runtime, Build, and Development

- Reworked the Node/Bun plugin runtime and Rust host bridge around one NDJSON protocol with
  deterministic registration, hook-failure reporting, diagnostics, and response-size limits.
- Added validation at plugin definition and configuration boundaries: a plugin requires a non-empty
  name and `register(api)` function, and invalid configured plugin objects fail during startup.
- Added plugin-aware source resolution for aliases, virtual modules, loading, transforms, lifecycle
  hooks, and dependency invalidation. Exact dependency aliases now carry
  source-specifier-to-resolved path bindings through compilation and dynamic-import chunking.
- Kept one TypeScript plugin worker/registry alive for the complete production build, so lifecycle
  and bundler hooks share initialization instead of restarting the runtime for each phase.
- Normalized development file-change notifications to project-relative paths and wired plugin hooks
  through native, standalone, and development execution paths without relaxing server/client
  boundary checks.

### Correctness and Security

- **Fixed stale client navigation pending state.** Concurrent route loads now use navigation IDs, so
  a completed older navigation cannot clear the pending state of a newer one.
- **Fixed binary Vercel responses.** The adapter now preserves response bytes by creating its body
  from `arrayBuffer()` data rather than decoding it as text.
- **Fixed package-manager detection on Windows.** `create-ruvyxa` supplies the process environment
  and uses shell execution for `.cmd` shims, allowing npm/pnpm commands to be detected correctly.
- Hardened plugin-controlled request rewrites: targets must be absolute application paths, percent
  decoded segments cannot introduce `/`, `\\`, `.`, `..`, controls, malformed encoding, or invalid
  UTF-8, and external URI targets are rejected.
- Hardened plugin scaffolding input validation by rejecting absolute and drive-prefixed `--dir`
  values and plugin names containing consecutive hyphens.

### Tooling, Documentation, and Release Reliability

- Added English and Thai plugin-authoring guides with the new API, lifecycle flow, socket selection,
  route matching, local package workflow, and HTTP/build examples. Added English and Thai error
  handling guides and updated CLI, configuration, architecture, official-package, demo, and guide
  navigation references.
- Migrated the demo plugins and configuration to the new API and expanded compiler, plugin, core,
  official-package, Vercel-adapter, router, and scaffolding test coverage for the new behavior.
- Updated release validation and package smoke coverage: version bumping synchronizes
  plugin-template peer and development dependencies, metadata validation rejects obsolete
  plugin-specific metadata, and tarball smoke tests scaffold, compile, and test a generated plugin
  package.
- Bumped Rust crates, npm packages, platform CLI packages, and starter templates to 1.0.24 while
  keeping workspace dependency ranges aligned.

### Benchmarks

- Refreshed the documented minimal-starter benchmark on Windows 11 Home, Ryzen 7 8845HS, Node.js
  22.23.1, npm 10.9.8, and pnpm 11.17.0. Across three cold-cache runs, Ruvyxa 1.0.24 recorded a
  1.848 s median production build, 1.020 s dev readiness, 0.828 s production readiness, and 44,316
  requests/second; exact Next.js and Astro conditions and limits are recorded in the README.
- Clarified that the benchmark compares minimal starter output, uses local packed artifacts for the
  unpublished Ruvyxa candidate, and is not a universal framework ranking.

## v1.0.23 (2026-07-26)

### Incremental Builds and Hydration Control

- Connected the persistent module graph to production client resolution. Warm builds reuse
  content-verified dependency edges, save graph state only after successful client emission, retain
  untouched entries when route artifacts hit, and invalidate the namespace when evaluated config or
  plugin dependencies change. Build telemetry now reports graph hits and tracked modules.
- Added route-level deferred hydration with `export const hydrate = 'idle'` and `'visible'` while
  preserving `true`/default eager hydration and `false` zero-JS output. Deferred pages share one
  content-hashed loader and do not module-preload the route bundle before its trigger. This is a
  route-level scheduling feature, not component resumability.

### Deployment Compatibility and Security

- Added a read-only adapter inspection protocol and expanded `ruvyxa doctor` with `--target`,
  `--adapter`, and `--json`. Doctor now reports adapter target/runtime/platform/capabilities and
  lists routes the selected deployment target cannot host before a build writes artifacts.
- Unified seven non-breaking security headers across native, standalone, and serverless responses;
  explicit application values retain precedence. Static and Cloudflare `_headers` output receives
  the same defaults. CSP and HSTS remain opt-in because framework-wide values would break valid
  inline bootstrap code or require deployment-specific HTTPS assumptions.
- Added `ruvyxa analyze --format sarif` with optional `--output`. SARIF 2.1.0 is serialized directly
  from existing `RUV####` diagnostics, preserving file locations, fixes, affected routes, and the
  command's non-zero exit status when violations exist.

### Production Build Performance

- **Fixed a responsive-image regression that increased the minimal production build from roughly 2
  seconds to 22 seconds.** The 2,000×2,000 starter image produces one full-size WebP and six
  responsive variants. Variant work was performed sequentially inside a source-level Rayon task, so
  a project with one large image used only one encoder path. Variant resize and WebP encoding now
  run in parallel while preserving deterministic manifest order, content-addressed cache keys, and
  output filenames.
- Extended the concurrency pass beyond responsive variants: full-size image encoding now overlaps
  variant work, asset/style/server preparation overlaps client bundling, and independent dynamic
  `getStaticParams` requests use the existing bounded worker pool instead of waiting route by route.
  Results and errors are reduced in deterministic order, and style files that can share an output
  path remain serialized after directory copies complete.
- Replaced static contiguous route-bundle chunks with a bounded dynamic work queue. Outer route
  workers claim the next available route while nested module resolution and compilation retain their
  separate Rayon pool, preventing an expensive route tail from leaving peer workers idle without
  recursively scheduling both levels in one pool.
- A clean `RUNS=3` comparison through `scripts/bench-frameworks.mjs` measured a **1.5 s** median
  Ruvyxa build, down from **2.1 s** after the first responsive-image fix and **22.1 s** before it.
  The same run measured Next.js 16.2.11 at 6.2 s and Astro 7.1.3 at 2.3 s. Ruvyxa still emits the
  complete responsive image set; the improvement does not disable optimization or remove variants.
- The v1.0.18 CLI built the same fixture in 1.2 s. The remaining difference is the cost of the
  responsive image outputs introduced after that release, rather than the 20-second serialization
  regression.
- A second clean audit run using locally packed, unpublished 1.0.23 packages measured a 1.8 s Ruvyxa
  build, 1.2 s dev readiness, 1.1 s production readiness, and 30,431 requests/second. The comparison
  used Next.js 16.2.12 and Astro 7.1.3; exact conditions are recorded in the README.
- Re-ran the clean benchmark after rebuilding the current release binary: Ruvyxa measured 1.609 s
  build, 1.123 s dev readiness, 1.056 s production readiness, and 41,991 requests/second. The same
  run measured Next.js 16.2.12 at 6.991 s / 3.903 s / 1.183 s / 3,653 requests/second and Astro
  7.1.3 at 2.363 s / 4.624 s / 1.867 s / 3,398 requests/second.

### Build and Scaffolding Correctness

- **Fixed incomplete builds leaving `.build-staging-*` directories behind.** Build staging now has
  an RAII owner from creation until commit, so every validation, bundle, prerender, plugin, and I/O
  error path removes partial output. A forced prerender failure verifies that no staging directory
  remains.
- **Fixed source checkouts scaffolding from an ignored, stale generated template.** `create-ruvyxa`
  now prefers tracked root templates when run from the monorepo and uses the packed template only
  after installation. Both preparation and copy boundaries exclude `.ruvyxa`, `dist`, and
  `node_modules`, preventing build output from leaking into newly created apps.
- Removed unused private linker and CLI parameters after tracing every caller; public and trait
  compatibility parameters remain intact.

### Dependency Compatibility

- Updated direct Rust `base64` usage from 0.22.1 to the latest stable 0.23.0 API. Axum and Oxc
  continue to bring 0.22 transitively until their own stable releases move forward.
- Updated Sass from 1.101.3 to 1.102.0. Registry checks found every other direct Rust and npm
  dependency already at its latest stable release; the Notify 9 line remains prerelease-only.
- Updated the pinned workspace package manager from pnpm 11.15.1 to 11.17.0 and verified the
  existing lockfile with the new version.
- Updated CI/release actions to `actions/checkout` v7, `actions/setup-node` v7, and
  `pnpm/action-setup` v6 so the automation dependency surface is current as well.

### Image Configuration Correctness

- **Fixed: documented `image.variantWidths` configuration was rejected as an unknown field.** The
  runtime config renderer now validates and forwards finite numeric arrays to the native CLI. Custom
  breakpoints work again, and `variantWidths: []` disables responsive variant generation as
  documented.
- Added config serialization coverage for `keepOriginal`, `variantWidths`, quality, lossless mode,
  and worker selection alongside the existing native image optimizer tests.

### Release Reliability

- The release workflow publishes every workspace package instead of relying on a recursive publish
  shape that could omit newly added packages. This prevents the main `ruvyxa` package from
  referencing an adapter version that was never uploaded to npm.
- Release jobs now verify every expected npm package and version after publication, turning a
  partial release into an explicit workflow failure instead of discovering it later through an
  application install error.

### Documentation and Verification

- Updated the README benchmark table, concurrency architecture, and methodology with the post-fix
  clean results, exact framework versions, cold-cache behavior, hardware, and the distinction
  between median startup measurements and the final throughput run.
- Verified with the complete `ruvyxa_cli` test suite, runtime compiler/config tests, TypeScript
  checks for `ruvyxa` and `@ruvyxa/core`, Rust formatting, Prettier, and the three-framework clean
  benchmark.
- Made `scripts/bench-frameworks.mjs` process cleanup portable and exception-safe. POSIX runs now
  own a detached process group, Windows uses tree/port cleanup only on Windows, and every readiness
  or load-test path terminates its server in `finally`, preventing stale listeners from corrupting
  later samples.

## v1.0.22 (2026-07-25)

### Four Additional Deployment Adapters

- Added `@ruvyxa/adapter-railway`, which emits a self-contained Railway service deployment with the
  standalone Node runtime and explicit deployment metadata.
- Added `@ruvyxa/adapter-render`, including Render Web Service and Blueprint-compatible output for
  deploying the generated standalone server.
- Added `@ruvyxa/adapter-firebase`, which packages static assets for Firebase Hosting and dynamic
  routes for Cloud Functions v2 while preserving Ruvyxa's route and rendering contracts.
- Added `@ruvyxa/adapter-aws`, which emits AWS Amplify Hosting static and compute artifacts for
  hybrid Ruvyxa applications.
- The CLI recognizes Railway, Render, Firebase, and AWS alongside the existing Node, Bun, static,
  Vercel, Netlify, and Cloudflare targets. The main `ruvyxa` package includes the new adapters in
  its deployment surface so configured and auto-selected builds use one adapter contract.

### Deployment Contract Alignment

- Extended shared adapter output types and standalone-server helpers for the four new platforms,
  keeping generated assets, client bundles, function handlers, and runtime metadata aligned with the
  existing deployment targets.
- Expanded adapter-runner validation and package smoke coverage so generated deployment artifacts
  are materialized inside the atomic build staging directory and required runtime files are present
  in package output.
- Updated realtime deployment guards and package guidance for targets whose server runtime can host
  the native self-hosted WebSocket transport.

### Documentation and Packaging

- Added dedicated Railway, Render, Firebase, and AWS package documentation plus an architecture
  reference for the full adapter matrix.
- Expanded the English and Thai deployment, CLI, plugin, Netlify, realtime, and static-adapter
  troubleshooting guides.
- Bumped all Rust crates, npm packages, platform CLI packages, and starter templates to 1.0.22 and
  synchronized workspace dependency ranges and the lockfile.

## v1.0.21 (2026-07-24)

### Packaging Fix

- **Fixed: `ruvyxa build` failed with `ERR_MODULE_NOT_FOUND` for `entry-templates.mjs`.** The
  `runtime/entry-templates.mjs` module — which `worker-pool.mjs` imports to compose route element
  trees — was missing from the `"files"` array in `packages/ruvyxa/package.json`. Published tarballs
  and local installs therefore never included it, causing the Node worker pool health check to crash
  immediately on `ruvyxa build` and `ruvyxa dev`. The file is now listed alongside the other runtime
  modules.

## v1.0.20 (2026-07-24)

### Client-Side Navigation

Ruvyxa route bundles already knew how to re-render into an existing React root; what was missing was
the half that decides _when_ to do so. That half now ships in `@ruvyxa/react`.

- **`<Link>` navigates without a document load.** It renders a real `<a href>`, so it stays
  crawlable, middle-clickable, and functional before hydration or with JavaScript off; the soft
  navigation is a progressive enhancement on top. Modifier-clicks, non-primary buttons, `target`,
  and `download` all fall through to the browser. Prefetch is configurable (`hover` by default,
  `viewport`, or off) and warms the target bundle with `modulepreload` — without executing it, so a
  prefetch can never register a tree built from the wrong parameters.
- **New hooks: `useRouter`, `usePathname`, `useParams`, `useSearchParams`, `useSelectedRoute`.**
  `useRouter` exposes `push`/`replace`/`back`/`forward`/`refresh`/`prefetch` and a `pending` flag
  for a route whose bundle is still loading. The routing context is created on `globalThis` so a
  generated entry can provide it without importing `@ruvyxa/react` — an app may render plain React
  pages and never install the package.
- **The browser matcher is a verified port of the server's.** `createRouteMatcher` in
  `@ruvyxa/react` shares one case table with the serverless handler's matcher
  (`tests/packages/react/route-match.test.mjs`), so a link click and a reload of the same URL always
  resolve to the same route and params, including static-over-dynamic precedence, catch-all
  decoding, and trailing-slash normalization.
- **The build publishes a lean `route-manifest.json`** the router fetches on first navigation —
  `{ path, src, sharedChunks }` per page route only. It deliberately is not `manifest.json`, which
  is a build report carrying absolute source paths that must never reach a browser. The dev server
  synthesizes the same shape at `/__ruvyxa/client/route-manifest.json`, so soft navigation works in
  development too. A missing manifest or an unmatched URL falls back to a full document load.

### Shared Route Composition

- **One source now composes every route's element tree.** The page-in-layouts-in-routing-context
  tree was re-implemented in five places (the Rust bundler, the dev server's SSR/SSG/client
  bundlers, the one-shot renderer, and the serverless registry). Composition now lives in
  `runtime/entry-templates.mjs` with a Rust mirror in `bundler/output.rs`, asserted equivalent by
  `tests/packages/ruvyxa/entry-templates.test.mjs`. A change to how routes are wrapped is a
  single-file change again, which is what makes the routing context reach the browser identically on
  every render path.

### Responsive Images

- **`<Image sizes=…>` now emits a real `srcset`.** For each public PNG/JPEG the build writes a
  downscaled `name-<w>w.webp` at every breakpoint narrower than the source, and `<Image>` builds its
  `srcset` from the same width list (`DEFAULT_DEVICE_WIDTHS`, matched to the optimizer's
  `DEFAULT_VARIANT_WIDTHS` and asserted equal in `tests/packages/react/image-variants.test.mjs`).
  The set is capped at the intrinsic width, so the browser never requests a variant the build did
  not produce. Configure the breakpoints with `images.variantWidths`; an empty array disables
  variants. A custom `loader`, `unoptimized`, or a remote/SVG source opts out untouched.

### Security: Open Redirect in the `redirects()` Plugin

- **Fixed: a wildcard redirect rule could send visitors to another origin.** The matched remainder
  of the request path was concatenated straight into the `Location` header, so with a rule such as
  `redirects([{ source: '/go/*', destination: '/*' }])` a request to `/go//evil.example` — or
  `/go/\evil.example`, which browsers fold the same way — produced `Location: //evil.example` and a
  cross-origin navigation. The remainder is request-controlled; the origin now is not.
  - A rule's reachable origin is fixed by its configured destination: an absolute destination pins
    its own origin, a path destination pins the requesting origin. Only path, query, and fragment
    may come from the request, and a rule whose interpolated destination would leave that origin is
    skipped instead of sent.
  - Destinations a browser reads as another origin (`*`, `//host`, `/\host`, non-http(s) schemes)
    are now rejected when the plugin is constructed, rather than at the first request that exploits
    them.
  - This is the same escape `safeReturnTo` blocks for `returnTo` in `@ruvyxa/auth`; the redirect
    plugin had been missed.

### Stability and Consistency

- **`--adapter bun` now emits a self-contained deployment.** It previously produced only a launcher
  that shelled out to `bunx ruvyxa start`, so a Bun host still needed the CLI and its native binary
  installed at runtime — unlike every other self-hosted target. Bun and Node now share one server
  source (`standaloneServerSource()` in `@ruvyxa/core`), so request ordering, static fallbacks, and
  cache headers cannot drift between the two runtimes. Run it with
  `bun .ruvyxa/deploy/bun/server/index.mjs`; the launcher is still emitted for the CLI workflow.
- **Fixed: a failed background refresh could freeze a cache entry as stale.** In `cache().swr()`, a
  refresh whose commit was rejected (the entry had been replaced or invalidated meanwhile) left the
  old entry flagged as refreshing, and no later reader ever started another refresh. The flag is now
  cleared when the commit does not land.
- **Fixed: the development auth stores grew without bound.** `memoryAuthStore` and
  `memoryRateLimitStore` only reclaimed a key when someone read it again, and rate-limit keys are
  derived from client IPs — one key per address, never read twice. Writes now sweep expired entries
  and evict oldest-first under a 10,000-entry ceiling.
- Every published package declares the same Node floor (`>=22.12.0`). Some advertised `>=22.0.0`
  while the framework they ship with requires `>=22.12.0`, so npm enforced a version that could not
  actually run the code. `pnpm release:validate` now fails on any package that disagrees.
- Published packages include `src`, so the shipped declaration maps resolve. `declarationMap` and
  `sourceMap` were on while `files` listed only `dist`, which pointed every "go to definition" and
  every stack frame at a file that was never in the tarball. `release:validate` enforces the
  pairing.

### Quality Gate

- `noUnusedLocals` and `noUnusedParameters` are enabled in `tsconfig.base.json`, giving the
  TypeScript packages the dead-code gate the Rust crates already get from
  `cargo clippy -- -D warnings`. The workspace passes today, so this only keeps it that way.

### Deployed Apps Now Behave Like `ruvyxa dev`

Five deploy-only failures, all of the same shape: a rule that only the Rust server enforced, so it
disappeared the moment a CDN or a platform bundler stood in front of the app.

- **Fixed: the Netlify function crashed on every request** with
  `ENOENT: no such file or directory, open '/var/task/manifest.json'`. Netlify re-bundles the
  function with esbuild and keeps only what the module graph reaches, so the sibling `manifest.json`
  that the handler read through `import.meta.dirname` never reached the deployed bundle. The route
  manifest now also ships as `manifest.mjs` and every adapter imports it statically — Netlify,
  Vercel, Cloudflare, and the standalone Node server. `included_files` cannot express this on the
  zero-config Frameworks API path, so removing the runtime read was the only host-independent fix.
- **Fixed: `public/` images 404'd on every static host.** Image optimization replaced
  `public/logo.png` with `logo.webp` in the build output, and only `ruvyxa dev`/`ruvyxa start`
  resolved the old URL to the new file. A CDN has no such fallback, so a plain
  `<img src="/logo.png">` broke in production only. The source file is now published beside its
  WebP; opt out with `image: { keepOriginal: false }` when every reference goes through `<Image>`.
- **Fixed: a missing asset returned `200` with an HTML body.** With no file behind it, `/logo.png`
  and `/favicon.ico` fell through to routing and were captured by a bare dynamic route such as
  `/[lang]`, so browsers received a page where they expected image bytes — and every favicon request
  paid for a serverless invocation in the function region. Asset-shaped paths now answer `404` in
  `dev`, `start`, and every adapter; routes that declare the extension themselves (`/sitemap.xml`)
  are unaffected.
- **Fixed: ISR and PPR pages never revalidated on Vercel or Netlify.** Their build-time HTML was
  published as a static file, and both hosts serve a matching static file before invoking the
  function (`handle: filesystem`, `preferStatic`), so the page was pinned to its deploy-time
  snapshot forever. Those pages are now withheld from the publish directory and kept inside the
  function bundle as the first cache entry.
- **Fixed: public assets were served with `max-age=0, must-revalidate` on Vercel**, so every
  navigation re-fetched each image and font. They now carry `public, max-age=3600, must-revalidate`,
  matching the header the Rust server already sent for the same files. Hashed client bundles keep
  their immutable header.
- `.ruvyxa-images.json` (build telemetry: source paths and byte counts) is no longer copied into the
  publish directory.
- Added `vercelAdapter({ regions: ['sin1'] })` to pin the serverless function near your users.
  Static pages are served from the edge, but SSR, API routes, and ISR revalidation run in the
  function region — `iad1` by default, a cross-continent round trip from Asia.

### The Same Audit, Applied to the Remaining Targets

- **Fixed (standalone Node server): `/logo.png` was answered by a page render.** The generated
  server routed before consulting the publish directory for everything except `/__ruvyxa/`, so a
  dynamic route captured the filename and the real file was unreachable. Asset-shaped paths are now
  resolved first, matching the Rust server's order.
- **Fixed (standalone Node server): public assets carried no `Cache-Control` at all**, and a
  PNG/JPEG URL did not fall back to the published WebP the way `ruvyxa start` does — so
  `image: { keepOriginal: false }` worked locally and 404'd in the shipped directory.
- **Fixed (Cloudflare): the Worker's `compatibility_date` was the build date.** Two builds of the
  same commit produced different Workers, and a build machine ahead of the deploy machine's
  `workerd` emitted a date `wrangler` rejects. It is now a fixed, tested default; override with
  `cloudflareAdapter({ compatibilityDate })`.
- **Fixed (Cloudflare): the Worker dropped the execution context**, so `waitUntil` was unavailable
  to anything the shared handler schedules in the background.
- **Fixed (static adapter): no `_headers` file was emitted at all**, so hosts that read one (Netlify
  drops, Cloudflare Pages) served even the content-hashed bundles with a revalidate-every-request
  default.
- Public-asset cache headers were extended to Netlify (`netlify.toml` and `.netlify/v1/config.json`)
  and Cloudflare (`_headers`), which both default to `max-age=0, must-revalidate` for
  publish-directory files.

Compatibility note: `deploy/bun/start.mjs` remains a launcher for projects that intentionally use
the installed CLI workflow. For a self-contained Bun deployment, use the server emitted at
`deploy/bun/server/index.mjs`; it shares the standalone server source with the Node adapter and does
not require the Ruvyxa CLI or native binary at runtime.

## v1.0.19 (2026-07-23)

### Deploy Anywhere: Static Linux Binaries

- Linux CLI binaries (`@ruvyxa/cli-linux-x64`, `@ruvyxa/cli-linux-arm64`) are now fully static musl
  builds. Releases before 1.0.19 were dynamically linked against the build machine's glibc and
  failed on hosts with an older glibc — most visibly Vercel's build image with
  ``/lib64/libc.so.6: version `GLIBC_2.39' not found``. The release pipeline now rejects any
  dynamically linked Linux artifact.

### Zero-Config Deploys Without Root Config Files

- `ruvyxa build` auto-detects the hosting platform from its build environment (`VERCEL`, `NETLIFY`,
  `CF_PAGES`) and runs the matching adapter when no adapter is configured. `RUVYXA_ADAPTER`
  overrides detection.
- All six official adapters are bundled with the `ruvyxa` package: `--adapter <name>` and platform
  detection work with zero installs. A project-installed adapter package still wins, and `--adapter`
  now also accepts any third-party adapter package name (`@scope/…` or `ruvyxa-adapter-…`), reported
  with the tried candidates in `RUV2203` on failure.
- The Netlify adapter now emits Netlify's Frameworks API directory (`.netlify/v1/`: the SSR/API
  function plus immutable cache headers) as a gitignored build artifact — no `netlify.toml` is
  written to the project root by default. Opt back in with
  `netlifyAdapter({ projectConfig: true })`.
- The Cloudflare adapter no longer writes a root `wrangler.jsonc` by default; the deploy directory
  is self-sufficient (`wrangler deploy -c .ruvyxa/deploy/cloudflare/wrangler.jsonc`). Opt back in
  with `cloudflareAdapter({ projectConfig: true })`.
- Fixed: opt-in `netlify.toml` and `wrangler.jsonc` previously embedded the absolute build-machine
  `outDir` (including a transient staging path and Windows backslashes), which broke Netlify deploys
  with a 404 on every route when the file was committed. Generated configs now embed
  project-relative POSIX paths only (`projectRelativeOutDir` in `@ruvyxa/core`).

### Standalone Node Server

- The Node adapter now emits a self-contained server at `.ruvyxa/deploy/node/server/index.mjs`
  (plain `node:http` around the shared serverless handler, static assets from `deploy/node/public`,
  `PORT`/`HOST` env, SSR/API/ISR/PPR/SSG/CSR). It runs on any Node host — Docker, PM2, systemd, any
  PaaS — with no ruvyxa CLI or native binary at runtime.

### Correctness

- Immutable cache headers for hashed client bundles now target the real URL prefix
  `/__ruvyxa/client/*` on Vercel, Netlify, and Cloudflare; the previous `/client/*` rules never
  matched, so hashed bundles were re-downloaded on every visit.
- `static-site` adapter artifacts can be marked `optional`, tolerating API-only builds with no
  prerendered pages instead of failing with `RUV2202`.
- Identical function bundles emitted at several destinations (deploy directory + platform discovery
  directory) are compiled once and copied, keeping build time flat.

## v1.0.18 (2026-07-22)

### Markdown Content Route Validation

- Boundary validation and rendering-strategy detection no longer treat fenced code blocks and inline
  code spans in `page.md`/`page.mdx` content routes as executable code. A guide that shows
  `process.env.SECRET` or `import 'server-only'` inside an example previously failed the build with
  false `RUV1007`/`RUV1008`/`RUV1009` diagnostics, and an example containing `fetch(` could silently
  demote a static page from SSG to SSR. MDX ESM outside fences is still validated.

### Bundler: Windows Path Normalization

- The package-`exports` resolver branch now strips Windows verbatim path prefixes (`\\?\`) the same
  way as every other resolver branch. Mixed prefixes previously broke shared-route chunk planning on
  Windows with `build.split: 'route'` in npm-layout projects, failing production builds with
  `prepared shared route module is unavailable: …\react\index.js`.

Both defects were found by building the Ruvyxa documentation site with the framework itself.

## v1.0.17 (2026-07-22)

### Official Data, Auth, and Realtime Packages

- Added `@ruvyxa/database`, a typed CRUD and transaction facade with Prisma-compatible, DynamoDB,
  and custom adapter contracts plus production environment validation.
- Added `@ruvyxa/auth`, with opaque durable sessions, credentials, OAuth PKCE, magic links,
  delegated WebAuthn, atomic replay/rate-limit contracts, and browser/server entrypoint separation.
- Added `@ruvyxa/realtime`, with action opt-in metadata, a bounded native Axum WebSocket transport,
  same-origin and channel filtering, reconnect/resync support, and explicit unsupported-target
  failures.
- The bundler and graph validator now treat root `@ruvyxa/auth` and `@ruvyxa/database` imports as
  server-only (`RUV1007`); browser code uses the `/client` entrypoints.

### Hardening

- Realtime transport paths are validated against reserved framework routes (`/__ruvyxa/hmr`,
  `/__ruvyxa/client`, `/__ruvyxa/action`, `/__ruvyxa/trace`) on both the TypeScript plugin runtime
  and the Rust dev server, failing configuration with a clear `RUV1701` diagnostic instead of a
  router panic at startup.
- The WebAuthn `options` endpoint now consumes the shared auth rate limit and reports failures
  through the same fail-closed error path as every other credential endpoint.
- The realtime browser client's `subscribe` no longer depends on `this` binding, so destructured
  usage (`const { subscribeRoute } = client`) works correctly.

### Plugin Infrastructure

- One `definePlugin({ name, setup })` registry now provides `resolveId`, `transform`, request and
  response middleware, and `onBuildComplete` hooks through a persistent Node/Bun plugin host, with
  NDJSON protocol isolation and per-plugin validation of names, hooks, and middleware route
  patterns.
- Middleware `routes` unions are reported to the native server, which skips the plugin round-trip
  entirely for requests no middleware can match.
- Added a configurable middleware worker pool (`middleware.workers`, 1–8) with round-robin dispatch,
  per-hook timeouts, crash restart with single retry, and replacement without retry on timeout or
  protocol errors.

### Content Engine and React Primitives

- Added the `contentEngine()` plugin: scans native `app/**/page.md(x)` routes once and derives
  `/content.json`, `/search-index.json`, `/rss.xml`, `/sitemap.xml`, and an experimental `/llms.txt`
  from frontmatter and body, live in development and byte-equivalent in production.
- Added the `Answer` component to `@ruvyxa/react` for schema.org Question/Answer microdata rendered
  from author-written content.
- SEO metadata API now supports `article` and `breadcrumbs` structured data, and `image`/`type`
  replace the previous `ogImage`/`ogType` property names.
- The render pipeline supports `header_pairs` so responses can carry multiple headers with the same
  name (for example several `set-cookie` values); header insertion appends instead of overwriting.

### Runtime and Tooling

- Persistent worker pool request handling was extended with a dedicated test suite and shared
  fixture workspace for API, compiler, and worker-pool tests.
- Serverless runtime adapters were expanded, including Cloudflare adapter updates and deployment
  documentation for every adapter target.
- `ruvyxa doctor` no longer reports a Deno version check; toolchain reporting focuses on the
  supported Node and Bun runtimes.
- Automated npm package smoke testing (`pnpm pack:smoke`) validates packed tarballs, template
  scaffolds, and Content Engine build artifacts.

### Performance: Static Serve Hot Path

- Production SSG responses are now served from the in-memory render cache after a single disk read,
  instead of re-opening the prerendered HTML file on every request. Measured on the minimal starter:
  ~1,300 → ~31,700 requests/second (p50 <1 ms) at 25 connections.
- The route manifest and radix router are shared via `Arc` instead of deep-cloned per request.

### Zero-JS Content Pages

- `export const hydrate = false` opts any server-rendered page (SSR, SSG, ISR, PPR) out of client
  hydration: the served and prerendered HTML contains no script tags and the production build emits
  no client bundle for that route. `'use client'` (CSR) pages ignore the export — the directive
  wins. Interactivity does not run on opted-out pages.

### Documentation and Benchmarks

- Added a measured benchmark suite against the Next.js and Astro minimal starters with a
  reproducible harness at `scripts/bench-frameworks.mjs`; results and methodology are published in
  the README.
- Added user guide chapter 15, "Official Packages: Database, Auth & Realtime" (English and Thai).
- Rewrote the Routing and Server & Client Components guides and expanded Getting Started with a
  first-10-minutes path and troubleshooting tables (English and Thai).

### Bundler: Resolution Cache

- Cache parsed `package.json` `exports` fields per package, fingerprinted by (mtime, len).
  Bare-specifier resolution (`react`, `react/jsx-runtime`, etc.) no longer re-reads and re-parses
  the same `node_modules` package.json for every importing module — the file is read once per build
  and served from cache thereafter, invalidated automatically if the file changes.

### Dev Server: Modular Architecture

- Split `ruvyxa_dev_server` into focused modules: `action_security.rs` (origin/fetch-metadata
  validation and per-key rate limiting), `cli_output.rs` (structured terminal formatting),
  `env_file.rs` (environment variable file parsing), `html_document.rs` (HTML document manipulation
  and template rendering), `plugin_bridge.rs` (plugin communication and lifecycle management),
  `port_binding.rs` (port availability detection and binding), and `static_assets.rs` (asset serving
  and caching strategies).
- Reduced `lib.rs` from ~1675 lines to ~108 lines of focused public exports, improving separation of
  concerns and maintainability.
- Extracted the rendering pipeline into `render_pipeline.rs` (SSR/SSG/ISR/CSR/PPR strategy dispatch,
  worker-pool render paths, ISR revalidation, and the render-process fallback), leaving `lib.rs`
  with server core only (config, serve loop, watcher, HTTP handlers).
- Response plugin middleware no longer fails oversized responses: a response whose sized body
  exceeds `plugin_response_body_limit_bytes` is now passed through unmodified (with a warning log)
  instead of returning a 500. Response plugins are skipped only for that response.
- Extended the oversized pass-through to unsized (streaming) response bodies: chunks are buffered up
  to the limit, and on overflow the already-read chunks are replayed in front of the untouched
  remainder so the response is served byte-identically instead of failing. Genuine body read errors
  still return a 500.

### Built-in Plugins and Middleware Fast Path

- Added `ruvyxa/plugins` package with first-party plugins: `redirects` (declarative 307/308
  redirects with wildcard remainders), `headers` (route-scoped response headers), `sitemap` and
  `robots` (build-time `sitemap.xml`/`robots.txt` generation from the route manifest), `alias`
  (exact import specifier resolution), `bundleBudget` (fail build when client JavaScript exceeds
  per-chunk or total KiB budgets), and `requireEnv` (fail build when required environment variables
  are missing or empty).
- Added native middleware fast path: the plugin registry reports middleware route patterns per
  direction, and the Rust server skips the plugin stdio round-trip for requests no middleware can
  match. Registries without request middleware no longer pay any per-request plugin cost.
- Added automatic plugin host recovery: when the persistent TypeScript plugin process crashes, the
  server restarts it once and retries the in-flight hook instead of failing subsequent requests.
- Added opt-in `middleware.workers` setting (1-8, default 1) for plugin runtime worker pool with
  round-robin dispatch and per-process crash recovery.
- Added the `ruvyxa/plugins` runtime alias for workspace and packed installs compatibility.
- Updated demo app to integrate `sitemap`, `bundleBudget`, and two-worker middleware pool as
  integration coverage.

### Runtime Rendering Consolidation

- Removed standalone `action-renderer.mjs`, `client-renderer.mjs`, and `ssg-renderer.mjs` modules.
- Consolidated all rendering operations (SSR, SSG/ISR/PPR, API, actions, client) into the persistent
  `worker-pool.mjs` process.
- Added `ssr-renderer.mjs` and `api-renderer.mjs` as standalone fallbacks when the worker pool is
  unavailable.
- Updated package manifests, smoke tests, and documentation to reflect the consolidated runtime
  architecture.

### Edge Runtime Bundle Target

- Added Edge bundle target variant for Cloudflare Workers and Vercel Edge Functions.
- Updated bundler to treat Edge bundles like SSR with server-side rendering but restricted Node.js
  APIs.
- Extended resolver to use `edge-light` condition for Edge target exports resolution.
- Added `serverless-handler.mjs` runtime for invoking Edge render functions.
- Updated adapter implementations (Vercel, Netlify, Cloudflare) to support full server rendering
  including SSR, API, and ISR routes on edge platforms.
- Added Edge runtime rendering tests across all three serverless adapters.

### Plugin Scaffolding Enhancements

- Added a custom plugin package directory option with path traversal protection.
- Changed default plugin output from `plugins/<name>` to root-level `<name>` directory.
- Generated plugin packages now include npm, pnpm, and Bun setup instructions in README templates.
- Added `scope` and `skipped` optional fields to adapter artifact reports for fine-grained build
  tracking.

### Platform and CI Improvements

- Normalized Windows path handling across bundler, CLI, dev server (HMR tracker, style modules), and
  diagnostics using `normalized_canonical_path()` utility.
- Expanded Bun runtime parity tests to Windows in CI workflow.
- Replaced environment variable runtime selection with explicit `--runtime` CLI flag for
  cross-platform consistency.
- Fixed Windows reserved port range handling (WSAEACCES 10013 errors from Hyper-V/WinNAT port
  exclusions) during dev server listener binding.
- Cleaned up unused `base64` dependency from `ruvyxa_middleware`.
- Improved npm package existence check reliability with Windows shell compatibility.

### Documentation

- Updated v1.0.16 release notes with comprehensive coverage of build output enhancements, server
  actions improvements, runtime detection, Bun support, progressive phase reporting, and CI/CD
  upgrades.
- Enhanced Thai CLI commands guide with detailed pipeline descriptions, `.ruvyxa/` output structure,
  `build.json` timing metadata, and command examples.
- Updated English and Thai plugin guides with built-in plugin documentation and middleware worker
  pool configuration.
- Updated deployment guides with Edge runtime serverless adapter capability matrix.

## v1.0.16 (2026-07-20)

### Plugin System Overhaul

- Replaced the split legacy plugin model with one TypeScript-native `definePlugin({ name, setup })`
  registry loaded from `ruvyxa.config.ts`.
- Added the typed setup API for `addMiddleware`, `resolveId`, `transform`, and `onBuildComplete`,
  with shared plugin state and deterministic registration order across server and build phases.
- Added `plugin(name, middleware)` as the compact authoring path for request/response middleware;
  `definePlugin({ name, setup })` remains available for plugins that also register build hooks.
- Added Fetch-native request and response middleware using standard `Request` and `Response` values;
  `undefined` continues, a returned `Request` replaces the request, and a returned `Response`
  short-circuits or replaces the response.
- Added route-scoped middleware matching with exact, wildcard, and prefix patterns, plus plugin
  context metadata containing the plugin name and project root.
- Added the persistent `runtime/plugin-runtime.mjs` Node/Bun registry process. It validates plugin
  setup, serializes hook results through NDJSON, redirects diagnostic logging to stderr, and keeps
  module-level state alive across calls.
- Added lossless request/response transport for binary bodies, query strings, duplicate headers, and
  repeated `Set-Cookie` values using ordered header pairs and base64 bodies.
- Added bounded response buffering through `security.pluginLimit` and Rust-side validation before
  converting plugin output into Axum responses.
- Added post-commit build completion execution so plugins can write deployment metadata and other
  artifacts after the production output is available.
- Replaced the public Rust bundler plugin trait with the internal `BuildHookPipeline` boundary and
  aligned resolver, compiler, source-map, and cache integration with the TypeScript host.
- Added the Rust `PluginHost` middleware bridge with process lifecycle management, descriptor
  validation, serialized hook errors, stderr forwarding, and graceful child cleanup.
- Removed Wasmtime, the raw Wasm ABI, Wasm plugin configuration, custom middleware layers, legacy
  plugin metadata (`enforce`, `parallel`, and hook flags), and the old `plugin-runner.mjs` worker.
- Simplified plugin scaffolding into a publishable npm package workflow with `src/index.ts`, package
  metadata, TypeScript build settings, and usage documentation.
- Updated package exports, runtime file manifests, keyword metadata, templates, configuration
  validation, README files, architecture references, English guides, and Thai guides for the new
  plugin lifecycle.
- Added focused coverage for plugin validation, persistent transform state, Fetch middleware, binary
  response preservation, repeated cookies, build completion, imported-plugin cache invalidation, CLI
  scaffolding, and Rust host protocol decoding.
- Removed orphaned Wasmtime dependencies from the workspace lockfile and verified packed npm output
  includes `runtime/plugin-runtime.mjs` without legacy runtime files.

### Built-in Plugins and Middleware Fast Path

- Added the `ruvyxa/plugins` package entry with first-party plugins built on the public hook API:
  `redirects` (declarative 307/308 redirects with wildcard remainders), `headers` (route-scoped
  response headers), `sitemap` and `robots` (build-time `sitemap.xml`/`robots.txt` generation from
  the route manifest into the served asset directory), and `alias` (exact import specifier
  resolution ahead of the native resolver).
- Added a native middleware fast path: the plugin registry now reports the union of middleware route
  patterns per direction, and the Rust server skips the plugin stdio round-trip — including request
  body base64 encoding and response buffering — for requests no middleware can match. Registries
  without request middleware no longer pay any per-request plugin cost, and older runtimes that do
  not report routes keep the previous match-all behavior.
- Added automatic plugin host recovery: when the persistent TypeScript plugin process crashes or its
  pipes break, the server restarts it once and retries the in-flight hook instead of failing every
  subsequent request. Hook-level errors are never retried.
- Added `bundleBudget` (fail the build when emitted client JavaScript exceeds per-chunk or total KiB
  budgets) and `requireEnv` (fail the build when required environment variables are missing or
  empty) to `ruvyxa/plugins`, and taught `sitemap` to read the committed route manifest when the
  build summary omits the route list.
- Added the opt-in `middleware.workers` setting (1-8, default 1): the server starts a pool of
  identical plugin runtime processes dispatched round-robin for middleware-heavy workloads, each
  with independent crash recovery. Module-level plugin state is per-process, so the default stays at
  one worker.
- Added the `ruvyxa/plugins` runtime alias so `ruvyxa.config.ts` can import built-in plugins inside
  the workspace and from packed installs, and wired the demo app to `sitemap`, `bundleBudget`, and a
  two-worker middleware pool as integration coverage.

### Large-Build and Content Compiler Follow-up

- Split route bundling into reusable prepare/emit stages so cold route-split builds resolve,
  compile, validate, and plan dynamic imports once, then perform only the final shared-aware
  link/minify/output pass.
- Added lightweight content-validated route-plan caching while preserving final artifact reuse;
  dynamic-import dependencies now participate in artifact invalidation instead of allowing stale
  lazy chunks after a source edit.
- Parallelized route preparation and final client emission while retaining deterministic
  manifest/output order and the existing `build.workers` bound.
- Replaced per-route dependency re-reading during warm artifact validation with one build-scoped,
  content-based fingerprint snapshot, preventing shared layouts and packages from being hashed
  repeatedly across large route sets.
- Replaced line-based MDX ESM extraction with markdown-rs MDX boundaries backed by Oxc syntax
  feedback, including multiline imports and exports.
- Combined MDX with GFM tables, task lists, strikethrough, autolink literals, references, and
  footnotes; added semantic table headings/alignment, reference resolution, stable duplicate heading
  slugs, JSX member/spread support, comments, and Markdown element component overrides.
- Upgraded frontmatter from a scalar line parser to locked `serde_yaml_ng` parsing for nested maps,
  arrays, quoted values, and block scalars, with actionable `RUV1312` failures for malformed or
  non-mapping documents.
- Aligned the packaged Node content compiler with the native contract using locked `yaml` and
  `remark-gfm` dependencies; Node SSR/SSG now preserves nested frontmatter, renders the documented
  GFM surface, and derives stable heading exports and rendered IDs from the same MDX AST.
- Added focused cache/concurrency regressions plus native MDX unit, full-bundler integration, and
  Node runtime parity coverage.
- Reused the first Oxc transform during Node module linking and added a bounded content-keyed
  transform cache, removing repeated work both within a graph and across identical route inputs.
- Memoized plugin-free native dependency closures, reused a production-build source snapshot, and
  cached successful native Markdown/MDX compilation results with bounded storage.
- Loaded prerender client assets once per build and shared immutable CSS across jobs instead of
  parsing the manifest and cloning the complete stylesheet for every route.
- Emitted the cold shared-route registry from prepared modules for plugin-free builds and persisted
  a fingerprint-validated warm artifact; shared source edits invalidate both the registry and
  affected route artifacts, while plugin builds retain their existing hook pass.
- Reduced the isolated 16-route demo benchmark from 13.61s to 4.02s cold and from 1.94s to a 1.62s
  warm median, with cold prerender down 89.2% and warm client bundling down 93.3%.

### Build Output and Release Profile

- Added progressive build phase reporting that displays real-time progress with per-phase durations
  for route discovery, validation, asset preparation, client bundling, and prerendering, so
  developers see timing as each stage completes rather than waiting for a single final summary.
- Added release profile optimizations (`thin` LTO, single codegen unit, symbol stripping) to
  `Cargo.toml` for smaller binaries, faster downloads, and improved runtime performance.
- Refactored build summary output into incremental metrics with a route size table and consolidated
  timing information for easier post-build inspection.
- Enhanced plugin scaffolding output with a visual file tree and numbered next steps for faster
  developer onboarding.

### Server Actions and Streaming

- Passed request headers through the server action rendering pipeline so Actions receive the
  originating `HeaderMap` via the worker pool and action renderer.
- Collected response headers from action handlers (`append`-style, multi-value) and propagated them
  back through the render pipeline to the HTTP response.
- Optimized the render cache recency tracking from O(n) linear queue scans to O(1) operations via a
  hash-indexed doubly linked list, replacing `VecDeque` with explicit `RecencyLinks` and
  `RecencyList`.
- Switched the API response stream from unbounded MPSC channels to bounded channels with capacity
  `MAX_PENDING_RESPONSE_FRAMES`, applying backpressure at the channel layer instead of manual queue
  overflow detection.

### Runtime Detection and Bun Support

- Added Bun as a selectable JavaScript runtime alongside Node, with `RUVYXA_RUNTIME` environment
  variable support for runtime override.
- Implemented `JavaScriptRuntime::detect()` to automatically select Node or Bun based on
  availability: Node is preferred, Bun is selected only when Node is unavailable and Bun can be
  executed, and Node is kept as the diagnostic target when neither runtime is installed.
- Extended `ServerConfig` and `ProjectConfig` with a `runtime` field (`"node"` or `"bun"`), and
  updated the worker pool, config renderer, and dev server to initialize with the selected runtime.
- Added the `@ruvyxa/adapter-bun` package for Bun-based deployment and launcher integration.
- Documented runtime configuration, automatic detection, and Bun parity guidance in English guides,
  Thai guides, README, and architecture references.

### Documentation

- Added comprehensive Thai CLI commands guide with structured sections, common options reference,
  detailed pipeline descriptions, `.ruvyxa/` output structure, `build.json` timing metadata, and
  command examples.
- Added system architecture reference guide spanning Rust/Node.js architecture, crate dependency
  maps, compilation pipeline stages, route graph algorithms, bundler resolution order, CSS Module
  handling, middleware plugin lifecycle, dev server hot reload, wire protocol specifications, error
  codes, and data flow diagrams.
- Added detailed architecture module guides for the bundler, CLI, concurrency, dev server,
  diagnostics, graph, middleware, protocols, security, and worker pool, with reference
  implementations and code examples.
- Removed archived architecture documentation (`build-performance-and-mdx.md`,
  `bundler-modernization.md`, `production-readiness.md`) after their content was integrated into the
  new architecture guides.

### CI/CD, Tooling, and Cleanup

- Rebranded CI/CD workflow and job names with consistent framework references and consolidated
  security scanning into primary workflows, removing the redundant standalone `security.yml`.
- Upgraded `pnpm/action-setup` from v4 to v5 and consolidated pnpm version management to the
  repository `packageManager` field as the single source of truth.
- Extended version-bump automation to iterate over all starter templates (`minimal`, `blog`, `crud`,
  `api-backend`) and validate framework dependencies across every template.
- Improved npm package existence check reliability with Windows shell compatibility and explicit
  error handling for `npm view` failures.
- Removed unused `anyhow` and `walkdir` dependencies from `ruvyxa_bundler`, `tower-http` from
  `ruvyxa_dev_server`, and `base64` from `ruvyxa_middleware` to reduce build footprint and
  transitive dependency counts.
- Bumped all npm workspace packages and Rust crates to `1.0.16` and regenerated `Cargo.lock` with
  synchronized dependency versions.

## v1.0.15 (2026-07-18)

### Full-System Reliability Hardening

- Hardened the API worker protocol so streamed responses require an explicit `api-end` terminal
  frame; premature EOF, worker crashes, and stream errors now reach the HTTP consumer instead of
  being reported as successful truncated responses.
- Preserved binary request and response bodies, query strings, duplicate request headers, and
  repeated `Set-Cookie` response headers across the Rust/Node worker boundary.
- Centralized request-path canonicalization to decode valid URL segments consistently while
  rejecting malformed escapes, encoded separators, traversal segments, and unsafe prerender paths.
- Fixed runtime-directory resolution for installations whose paths contain spaces or other
  URL-encoded characters by using filesystem-safe URL conversion throughout the Node runtime.
- Made automatic JSX the consistent default across the Rust bundler, CLI, dev server, and Node
  renderers; classic JSX remains available as an explicit opt-in.
- Validated JSX runtime configuration at startup and linked the generated `react/jsx-runtime` helper
  imports correctly in SSR, SSG, client, and worker bundles.
- Extended package `exports` resolution with target-aware conditions, wildcard subpaths, array
  fallbacks, explicit blocked entries, package-root containment checks, and safer filesystem
  fallback behavior.
- Preserved the server/client boundary and private environment-variable checks while improving
  resolver and compiler cache invalidation behavior.
- Corrected CORS ordinary `OPTIONS` handling, preservation of all `Vary` values, trusted-proxy
  forwarding rules, loader request lifecycle handling, cache-duration validation, and related
  middleware/runtime regressions.
- Updated CLI/configuration documentation and the full-flow smoke script to match the maintained
  `examples/demo` fixture and current JSX defaults.

### Client Bundling Reliability

- Fixed the Node runtime compiler's client-module initialization order. It now performs a stable
  dependency-first traversal instead of reversing module discovery order, which was not a valid
  topological order when separate graph branches shared React or another dependency.
- Prevented client components that import React hooks from failing at `/__ruvyxa/client` with
  `Cannot access '__m…' before initialization` during development or hydration bundle evaluation.
- Added a runtime compiler regression that reproduces the cross-branch shared-dependency graph and
  evaluates the generated bundle to prove every acyclic local dependency initializes before its
  importers.
- Kept the Node runtime behavior aligned with the Rust bundler's existing dependency-first linker
  without changing compiler APIs, entry exports, module identifiers, or source-map behavior.

### Release Metadata and Documentation

- Bumped all npm workspace packages and Rust crates to `1.0.15` and regenerated `Cargo.lock`.
- Updated the minimal starter to require both `ruvyxa` and `@ruvyxa/react` `^1.0.15`.
- Updated the version-bump workflow so future releases keep both starter framework dependencies in
  sync; the ignored `create-ruvyxa` package copy continues to be regenerated from the source
  template during prepack.
- Documented the client initialization root cause, applied repair, and regression evidence in the
  July reliability audit.

### Stability and Compatibility Follow-up

- Fixed Node worker environment parsing so values with trailing units or other extra characters,
  such as `1234ms` and `64mb`, are rejected and safely fall back instead of being partially parsed.
- Preserved conditional `package.json` `exports` key declaration order to match Node resolution
  semantics without changing JSON ordering behavior elsewhere in the workspace.
- Assigned the unique `RUV1804` diagnostic code to invalid JSX runtime configuration, keeping
  `RUV1803` reserved for circular dependency diagnostics.
- Added regression coverage for malformed worker configuration, invalid JSX runtime diagnostics,
  conditional exports declaration order, early API-stream termination, encoded URL boundaries, and
  cross-runtime JSX helper linking.
- Revalidated the release surface: 325 Rust tests, workspace clippy with warnings denied, npm
  build/check/test, demo parity for all 16 routes, package metadata validation, and packed-package
  consumer type checks all pass on Windows x64.
- No tracked critical files were deleted or missing, and no dependency was removed without direct
  evidence of being orphaned; generated build, cache, and package-smoke outputs remain excluded from
  the tracked release surface.

## v1.0.14 (2026-07-16)

### Reliability and Configuration Safety

- Normalized `RUVYXA_WORKER_TIMEOUT_MS` and `RUVYXA_MEMORY_LIMIT_MB` in the persistent Node worker:
  invalid or zero values now safely retain the 30-second watchdog and 512 MiB cache-pressure
  threshold instead of silently disabling protection.
- Aligned the Rust worker-response and API stream-idle timeout with the normalized
  `RUVYXA_WORKER_TIMEOUT_MS` value passed to Node. Interactive requests now consistently use the
  documented 30-second fallback, while build workers retain their 300-second fallback unless
  explicitly overridden. Values above Node's 2,147,483,647 ms timer limit now fall back safely
  instead of being coerced by Node to a 1 ms timeout.
- Bounded environment-derived `RUVYXA_RENDER_CACHE_SIZE` at 16,384 entries before render-cache
  allocation, while preserving `0` as an explicit cache-disable setting and preserving existing
  development and production defaults.
- Added regression coverage for worker environment fallback and render-cache capacity normalization.
- Streamed API response bodies from Node workers into Axum with binary-safe 64 KiB Base64 frames, a
  bounded 16-frame per-response queue, idle timeouts, stdout backpressure, and stream error
  propagation instead of materializing each response as one text value.
- Kept the API worker protocol backward-compatible: new Rust callers accept legacy single-message
  responses, while new Node workers retain that response shape unless streaming is requested.
- Added Rust and Node regressions for binary reconstruction, large multi-frame responses, queue
  overflow, stalled streams, worker errors, request capability serialization, and legacy fallback.
- Corrected the README cache description from FIFO to its implemented LRU policy and documented the
  supported worker/cache environment settings in English and Thai CLI guides.
- Refreshed the July reliability audit with current v1.0.14 bundler context, applied repairs, and
  the completed streaming API-response IPC repair.

### Bundler and Build Pipeline

- Added shared module bundling and a shared-route registry so modules common to multiple routes can
  be compiled once and reused instead of duplicated in every client bundle
- Added `bundle_shared_route_modules()` and shared-route output types for producing executable
  shared module registries
- Added linker support for shared modules, dynamic imports, dependency-first linking, and exclusion
  of already-emitted shared modules from individual route bundles
- Added `collect_module_manifest` and improved static-module tracking for more accurate chunk and
  module manifests
- Integrated shared-module output with the CLI build pipeline, render cache, and development server
- Added async build phases for route discovery, validation, preparation, client bundling, and
  prerendering
- Added per-phase timing metrics and total build duration reporting in build output metadata
- Added a prerender worker pool that chooses parallelism from route count and available CPU capacity
- Migrated static prerendering and SSG rendering to the async worker-pool workflow
- Replaced the duplicated hand-written TypeScript stripping and JSX lowering paths in both the Rust
  bundler and `runtime/compiler.mjs` with Oxc 0.139.0 transformers
- Preserved the existing resolver, graph cache, plugin ordering, linker, module metadata, client
  boundary validation, and public compile APIs while moving syntax transformation behind narrow Oxc
  adapters
- Added Oxc semantic analysis before Rust-side transformation so TypeScript enums, namespaces,
  `satisfies`, typed destructuring, JSX fragments, spread props, and namespaced JSX tags continue to
  compile through one parser-backed pipeline
- Kept classic React JSX output as the compatibility default and retained the automatic JSX runtime
  option without changing caller-facing compiler configuration
- Retained the Rust bundler's historical decorator behavior with a compatibility pre-pass, avoiding
  unresolved `@oxc-project/runtime` helper imports until helper-aware graph integration is
  introduced
- Removed Node's experimental `stripTypeScriptTypes` dependency and the custom runtime
  `JsxTransformer`; all Node renderers now reach the same Oxc-backed compiler entry points
- Pinned the Rust and npm transformer implementations to Oxc `0.139.0` and included native bindings
  for supported Windows, macOS, Linux, and WASI targets in the package lock
- Raised the framework, workspace, demo, and starter app Node requirement from `22.0.0` to `22.12.0`
  to match the native Oxc transformer runtime contract
- Improved resolver, compiler, and graph-cache reuse across multi-route builds
- Rebranded native bundler references to **Ruvyxa Bundler** across diagnostics, documentation, and
  package metadata

### Runtime and Developer Experience

- Improved worker-pool lifecycle and prerender reliability for production builds
- Added consistent millisecond-duration reporting for build and render phases
- Improved runtime worker-pool coordination for asynchronous route rendering
- Added clearer file I/O errors that include the missing source path, making dependency and package
  setup failures easier to diagnose
- Simplified the path-aware resolver read helper so strict workspace Clippy passes without the
  redundant enclosing `Ok(...)` and `?`, while preserving the original I/O error kind and path
- Updated compiler and worker-pool regression coverage for the new asynchronous execution model
- Expanded compiler parity coverage across Rust parser fixtures and the published Node runtime. Rust
  fixtures cover annotations, enums, decorators, fragments, spreads, and nested expressions; Node
  runtime tests cover enum and namespace lowering, TSX, CSS-in-JS objects, dynamic imports, cache
  invalidation, source maps, and paths containing spaces
- Added cross-platform project setup scripts:
  - `setup.bat` with the complete Windows setup workflow
  - `setup.sh` with the complete macOS/Linux setup workflow
- Setup now installs locked workspace dependencies, builds all npm workspace packages, and compiles
  the Ruvyxa CLI before development, without depending on a shared `setup.mjs` launcher

### Release and Documentation

- Bumped workspace packages and Rust crates to `1.0.14`
- Updated English and Thai CLI documentation for shared bundling, async builds, and prerender
  parallelism
- Updated English and Thai configuration documentation for the new build behavior
- Updated bundler architecture, developer, package, and production-readiness documentation
- Documented the Oxc ownership boundary, decorator compatibility strategy, source-map follow-up, and
  native Node version requirement
- Added and updated compiler, parser compatibility, shared bundling, and worker-pool regression
  coverage

## v1.0.13 (2026-07-14)

### Runtime Path Compatibility

- Fixed runtime alias resolution when Ruvyxa is installed in a path containing spaces or other
  URL-encoded characters by using `fileURLToPath()` across standalone renderers, the worker pool,
  and the runtime compiler
- Added regression coverage that loads the runtime compiler from a temporary path containing spaces

### Server Reliability and Forwarded-Header Security

- Fixed server startup so action endpoints receive Axum TCP connection metadata instead of failing
  `ConnectInfo` extraction at runtime
- Restricted forwarded client and protocol headers to loopback or explicitly configured
  `security.trustedProxyIps`, preventing private-network clients from bypassing action rate limits

### Release Metadata and Templates

- Bumped all npm packages and Rust crates to `1.0.13`
- Updated both minimal starter template copies to require `ruvyxa` and `@ruvyxa/react` `^1.0.13`

## v1.0.12 (2026-07-13)

### Oxc Integration and Modernized Minification

- Integrated the Oxc 0.139.0 ecosystem (parser, semantic minifier, mangler, code generator) for
  production JavaScript minification, replacing the selective token compressor
- Oxc owns final parsing, semantic compression, name mangling, and minified code generation while
  Ruvyxa retains framework-specific resolution, linking, boundary checks, and output composition
- `build.treeShaking` keeps its public meaning: enabled uses Oxc full compression; disabled uses
  `CompressOptions::safest()` to preserve unused bindings
- Removed the old selective token compressor from production code paths; `minify_parallel` now
  delegates to a single whole-program Oxc pass since semantic mangling cannot be performed per
  linker segment
- Removed unused `compile_graph`, `CompilerError`, `compile_graph_resilient`, and
  `parse_error_location` utilities, simplifying the compiler public API
- Added `base64-simd`, `compact_str`, and `num-bigint` for performance-critical operations in the
  bundler pipeline
- Published `docs/architecture/bundler-modernization.md` documenting the oxc integration boundary,
  adoption map, and next safe stages

### Linker and Resolver Improvements

- Added CommonJS `module.exports` / `__exports` / `process.env` shims for compatibility with
  CommonJS bundles expecting Node.js globals; `process.env` stubs default to production
- Implemented tsconfig/jsconfig content fingerprinting and cached path resolution to avoid repeated
  I/O and parsing across multi-file builds; invalidates cached configuration on file modification
- Added support for `import Default, * as ns` import clause combinations
- Introduced `is_identifier()` utility for robust identifier validation in import clause parsing
- Converted the linker's `rewrite_module_into`, `try_rewrite_import`, and `rewrite_import_clause` to
  return `Result` types for consistent error propagation through `link_inner` and `link_parallel`
- Skipped dynamic chunk generation when `emit_chunk_manifest` is disabled to avoid unnecessary
  processing

### Packaging

- Bumped all npm packages and Rust crates from 1.0.11 to 1.0.12
- Applied consistent compact array syntax to `files`, `keywords`, `os`, and `cpu` fields across all
  platform CLI packages, adapter packages, core, react, and create-ruvyxa package manifests

### Security: Wasm Plugin Response Buffering Limits

- Added `security.pluginLimit` configuration option defaulting to 32 MiB (max 256 MiB) to control
  response-phase Wasm plugin body buffering, preventing unbounded memory growth
- Introduced `MAX_PLUGIN_RESPONSE_BODY_LIMIT_BYTES` constant and RUV1602 diagnostic for invalid
  limits; zero and beyond-maximum values are rejected at config load
- Propagated `plugin_response_body_limit_bytes` through `ServerConfig` into both dev and production
  server paths, applying the limit at the Axum body extraction layer
- Updated user guide with plugin buffering limits, memory considerations, and configuration examples
- Added validation tests for zero, within-range, at-maximum, and over-maximum limit values

### Developer Experience: Pre-commit Hook

- Added `.githooks/pre-commit` hook that runs `format-staged.mjs` before every commit, verifying
  Prettier formatting for staged JS/TS/JSON/MD files and `cargo fmt --check` for staged Rust files
- Created `scripts/format-staged.mjs` to detect changed files, run the appropriate formatter, and
  block commits that would fail CI formatting checks
- Added `scripts/setup-git-hooks.mjs` and a `prepare` lifecycle script so hooks activate
  automatically on `pnpm install`
- Added `format:staged` package script for manual on-demand staged-file formatting
- Updated `CONTRIBUTING.md` to document the pre-commit hook behaviour

### Documentation: User Guide Restructuring

- Replaced the single `docs/user-guide.md` (517 lines) with an organized `docs/guides/` directory
  containing 12 focused chapters per language
- Added complete **English** guides: getting started, routing, server/client components, API routes,
  data loading and cache, server actions, rendering strategies, markdown/MDX/images, environment
  variables, configuration reference, CLI commands, and deployment
- Added complete **Thai** (ภาษาไทย) translations alongside every English chapter under
  `docs/guides/th/`
- Created `docs/guides/index.md` with a bilingual table of contents, language selector, and quick
  navigation section for application authors
- Updated `README.md` Documentation section with a linked table pointing to all four doc resources
  (User Guide, Developer Guide, Bundler Modernization, Production Readiness) and moved it higher for
  visibility
- Updated `developer-guide.md` links to point to the new guide index
- Updated documentation to reflect current system defaults: added `preview` and `bench` CLI
  commands, `parity` alias, `pluginLimit` security option, `plugins` and `middleware` config fields,
  explicit Rust 1.96+ requirement, and correct `middleware.builtin.log` / `middleware.builtin.rate`
  field names

## v1.0.11 (2026-07-12)

### macOS x64 Native Binary Removal

- Removed `@ruvyxa/cli-darwin-x64` package directory and configuration
- Removed `darwin-x64` from `supportedPlatforms` mapping in `scripts/native-platform.mjs`
- Removed `@ruvyxa/cli-darwin-x64` optional dependency from main package
- Updated error message in `bin/ruvyxa.js` to reflect remaining 5 supported platforms
- Added test case verifying `darwin-x64` is not published or resolved
- Intel macOS support discontinued in favor of ARM64 architecture

### Production Minification and CSS Optimization

- Replaced the third-party minification bypass with token-aware compression for the complete client
  bundle, including `node_modules`
- Preserved regular expressions, strings, template literals, legal comments, and JavaScript
  automatic-semicolon-insertion boundaries during compression
- Folded CommonJS `process.env.NODE_ENV` guards while resolving production client dependencies so
  React and similar packages include production implementations without development branches
- Updated module labeling in linker to use full paths consistently
- Added CSS minification support with `minify_css()` in dev server for production builds while
  preserving readable CSS in watch mode
- CSS minifier strips comments and collapses whitespace, preserving string/`url()` content

### Rate Limit Bypass Prevention and Worker Reliability

- Extracted peer socket address in action endpoint to capture direct client IP
- Implemented trusted proxy detection to prevent `X-Forwarded-For` spoofing attacks
- Only trust forwarded headers when direct peer is loopback or private address
- Added idempotent request detection to safely retry only SSR, SSG, and client requests
- Quarantined failed workers to prevent processing conflicting retry requests
- Added stderr drain task to prevent Node worker process pipe buffer overflow
- Implemented sliding-window rate limiter middleware with per-client IP tracking
- Improved worker pool fallback messaging to clarify idempotent request retry logic

### Documentation Consolidation

- Reorganized docs structure into two main guides: `docs/user-guide.md` for app developers and
  `docs/developer-guide.md` for framework contributors
- Deleted specialized docs (getting-started, routing, content-and-images, data, actions, deployment,
  debugging, performance, parity, production-readiness, publishing, architecture/project-structure)
- Updated README.md documentation links to point to the two new consolidated guides
- Added demo app README with health check example
- Updated CONTRIBUTING.md to reference new documentation structure
- Simplified documentation maintenance by centralizing content into purpose-specific guides

### Smoke Test and Script Improvements

- Isolated scaffolded app workspace context in smoke tests by creating empty `pnpm-workspace.yaml`
- Overrode smoke test dependencies with local tarballs for comprehensive validation
- Added pnpm overrides for transitive dependency resolution during smoke tests
- Added tarball resolution for `@ruvyxa/core`, `@ruvyxa/react`, and platform-specific CLI packages
- Improved smoke test isolation by using system temp directory instead of hardcoded path
- Removed redundant `ruvyxa` type declaration from minimal template `tsconfig.json`
- Simplified type resolution by relying on `ruvyxa` package's included types

### Infrastructure

- Removed `.githooks/pre-commit` hook for Cargo.lock validation (now handled through CI/CD)
- Suppressed clippy `too_many_arguments` warning on `print_build_report` function

### Windows arm64 Support

- Added `@ruvyxa/cli-win32-arm64` platform package with native CLI binary for Windows arm64
- Extended supported platform mapping in `scripts/native-platform.mjs` to include `win32-arm64`
- Updated `nativeBinaryPackageName()` — all supported platforms are now resolved through a shared
  data module instead of a hardcoded switch
- Added Windows arm64 to the CI build matrix (`.github/workflows/ci.yml`,
  `.github/workflows/release.yml`)
- Updated binary resolution in `bin/ruvyxa.js` to display `win32-arm64` in the supported-platforms
  message and route to the new optional package
- Added `@ruvyxa/cli-win32-arm64` as a dependency in `ruvyxa/package.json`
- Added native platform test suite (`native-platform.test.mjs`) verifying the mapping, package
  metadata, and unsupported-platform fallback

### Security Configuration

- Added `security.apiLimit` configuration for maximum API route request payload size (default: 10 MB
  / 10,485,760 bytes)
- Added `security.actionRateLimit` with `max` (default: 600) and `window` (default: 60s) for
  configurable per-client/action rate limiting
- Raised default `actionLimit` from 64 KB to 1 MB (1,048,576 bytes)
- Raised default action rate limiter from 60 req/min to 600 req/min
- Added `RUV1601` config validation for zero-valued security limits (`actionLimit`, `apiLimit`,
  `actionRateLimit.max`, `actionRateLimit.window`)
- Added strict unknown-field rejection for `config.security.actionRateLimit`
- Extended TypeScript types in `@ruvyxa/core` with `apiLimit` and `actionRateLimit` fields
- Forwarded new security config fields through runtime config renderer (`config-renderer.mjs`) and
  into production `build.json` output
- Updated security section in all documentation to reflect new keys and defaults

### Server and Worker Pool Lifecycle

- **Graceful server shutdown** — intercepts SIGTERM / Ctrl+C, notifies workers, and terminates with
  a 5-second grace period before force-closing remaining connections
- **Worker pool shutdown** — added `NodeWorkerPool::shutdown()` that closes stdin on every worker,
  clears pending requests, and force-terminates workers that do not exit within 2 seconds
- Worker stdin access now uses a `Mutex<Option<mpsc::Sender>>` so senders are safely drained during
  shutdown; operations after shutdown return a clear `"Worker process is shutting down"` error
- Worker `_child` made accessible via `Mutex<Option<Child>>` to support `kill` + `wait` on shutdown
- HMR client script simplified — now always issues `location.reload()` for every update, eliminating
  the fragile targeted CSS/component refresh code path
- Security headers no longer inject `Connection: keep-alive` / `Keep-Alive: timeout=30, max=1000`
  into every response; WebSocket `Connection: Upgrade` headers are preserved

### Config Validation and CLI

- Added `validate_positive_limit()` helper raising `RUV1601` for zero-valued numeric limits
- Added Rust tests for zero-limit rejection on `apiLimit` and `actionRateLimit`
- Updated existing security config tests to verify new `apiLimit` / `actionRateLimit` fields
- `config()` shorthand key table in getting-started docs updated with `apiLimit` and
  `actionRateLimit`

### Compiler and Runtime

- Runtime compiler (`compiler.mjs`) now rewrites named `export class` declarations before wrapping
  modules, making class exports available after module wrapping
- Added compiler test for named class export rewriting with runtime verification

### create-ruvyxa

- Scaffolded projects now receive their own `package.json#name` derived from the target directory
  name (sanitized to a portable npm package name)
- Added `toPackageName()` and `writeProjectPackageName()` helpers in `create-ruvyxa/src/index.ts`
- Added test coverage for package-name derivation and output verification

### CI and Infrastructure

- Added Ubuntu 24.04 ARM64 to the CI and release build matrix
- All npm packages, Rust crates, lockfiles, and template dependencies synchronized

### Documentation

- Documented `security.apiLimit` and `security.actionRateLimit` config keys across all guides
- Updated security defaults (1 MB action limit, 10 MB API limit, 600 req/min rate limit) in actions,
  deployment, production-readiness, and publishing docs
- Added `@ruvyxa/cli-win32-arm64` to native binary platform tables in production-readiness,
  publishing, deployment, and project-structure documentation
- Updated CI/CD documentation to reflect Windows arm64 and Ubuntu ARM64 build runners
- Updated build metadata example in deployment docs with new security fields
- All concise config key tables reflect the current configuration contract
- Version and dependency references updated across the documentation set

## v1.0.10 (2026-07-11)

### Content, Images, and SEO

- Added first-class `page.md` and `page.mdx` routes with frontmatter, heading metadata, GFM
  Markdown, MDX ESM imports, JSX components, expressions, SSG, and HMR support
- Shared content compilation across Ruvyxa Bundler and Node runtime compiler, including
  content-aware dependency scanning that ignores imports inside fenced code examples
- Added `frontmatter`, `meta`, `headings`, and `contentFormat` exports to generated content modules
- Rebuilt image optimization around a single-output `.webp` pipeline that replaces local PNG/JPEG
  asset extensions instead of generating AVIF/WebP sidecars beside the original files
- Optimized public assets in one parallel pass with persistent content caching, direct cache reuse,
  collision detection, and unchanged fallback copies for invalid or non-image files
- Simplified development and production image serving so `.webp` assets resolve directly, while
  legacy local PNG/JPEG requests can still map to the optimized `.webp` output where applicable
- Added compact image manifest output with source/output paths, dimensions, byte sizes, source
  bytes, output bytes, optimized image counts, and cache hit tracking
- Updated typed image configuration to `image.optimize`, `image.quality`, `image.lossless`, and
  `image.workers`
- Upgraded `@ruvyxa/react` images with local-only `.webp` rewriting, `fill`, author-managed
  `srcSet`, browser-native `Picture` art direction, loading controls, and per-image CDN loaders
  without adding runtime image transformation

### Hashing and Build

- Upgraded asset hashing from BLAKE3-64 to BLAKE3-256: `content_hash()` now returns the full
  64-character hex output instead of a truncated 16-character value; `ASSET_HASH_ALGORITHM` constant
  changed from `"blake3-64"` to `"blake3-256"`
- Updated `build.json` hash algorithm output and documentation to reflect 256-bit hashing
- Client bundle file names now use full BLAKE3-256 content hashes for stronger cache uniqueness

### CLI and Config

- Replaced `defineConfig()` with `config()` and adopted concise configuration keys across the public
  contract; `appDir` and `outDir` remain unchanged
- Added `debug.traces` configuration option for debug trace control in the dev server
- Added `deny_unknown_fields` to `ProjectConfig` and `DebugConfigOptions` for strict config
  validation against unknown keys
- Added strict top-level config validation for `runtime`, `react`, `typescript`, `render`, `image`,
  `security`, `cache`, `middleware`, `adapter`, `adapterOptions`, and `plugins`
- Implemented `normalize_source_path()` to gracefully handle non-existent paths in HMR tracking
- Fixed Windows watcher paths prefixed with `.` so generated `.ruvyxa` cache writes are ignored
  instead of triggering repeated reloads; condensed dev startup and HMR logs into readable summaries
- Added concise dev document-request logs with method, route, response status, and sub-millisecond
  timing while excluding HMR and static asset traffic
- Updated worker pool and config renderer with improved runtime implementations
- Added tests for asset hash algorithm, dev config overlay/trace flags, unknown field rejection, and
  HMR tracker path normalization

### Branding and Error Page

- Centralized the framework logo at `assets/branding/ruvyxa.png` as the canonical source
- Added `assets/branding/README.md` documenting synchronization of runtime copies across starters
  and the error page
- Refined the plain error page into a centered 404/500 recovery layout with logo, status code,
  title, and escaped diagnostics on a dark outer background with white card and purple accent

### Infrastructure

- Added `.githooks/pre-commit` hook validating `Cargo.lock` synchronization before commits
- Added `scripts/check-cargo-lock.mjs` script and `check:cargo-lock` npm script for manual
  validation
- Upgraded Rust workspace from edition 2021 to 2024 and resolver from "2" to "3"
- Applied `cargo fmt` with Rust 2024 formatting rules across all crates
- Upgraded Rust dependencies: cranelift 0.132.2→0.133.1, tower-http 0.6.11→0.7.0, pulley
  45.0.2→46.0.1, mach2 0.4.3→0.6.0, wasm-compose/encoder/parser to 0.251.0
- Upgraded bytes 1.11.1→1.12.0, cc 1.2.64→1.2.65, log 0.4.32→0.4.33, quote 1.0.45→1.0.46
- Upgraded Node.js package versions across all workspace packages and regenerated lockfiles

### Diagnostic Codes

- Added `RUV1101` SSR renderer args missing diagnostic
- Added `RUV1550` PPR (Partial Prerendering) render failed diagnostic
- Added `RUV1801` Module resolution error diagnostic
- Added Partial Prerendering (PPR) error code section to diagnostics guide
- Refined error code table formatting and alignment for readability

### Testing

- Added `worker-pool.test.mjs` test suite for worker pool behavior
- Expanded compiler tests with content compilation, fenced-import handling, and image configuration
  coverage
- Added tests for React metadata, route discovery, dev/prod parity, error-page escaping and layout
- Added regression coverage for the new single-output `.webp` optimizer, cache reuse, collision
  rejection, invalid image fallback, disabled optimization, and dev server `.webp` source resolution
- All existing test suites updated and passing

## v1.0.9 (2026-07-10)

### Client Bundling and Boundaries

- Bundled browser React and React DOM dependencies, including CommonJS package dependencies, so
  client hydration no longer leaves unresolved bare `react` module specifiers
- Preserved valid third-party JavaScript, including regular-expression literals, when the native
  text minifier cannot safely parse the dependency source
- Made server/client boundary diagnostics syntax-aware so ordinary content containing `server-only`
  is not treated as a module marker
- Ignored type-only imports during runtime dependency resolution

### Build Reliability

- Capped default and configured static pre-render concurrency at two workers to prevent memory
  exhaustion on content-heavy sites
- Added Windows rename retries for transient file locks while committing build output
- Fixed file-watcher cache invalidation on threads without a Tokio runtime

### Starter and Documentation

- Added the CSS module declaration required by the minimal TypeScript starter
- Synchronized all npm packages, Rust crates, lockfiles, and template dependencies to 1.0.9
- Added regression coverage for client dependency bundling, boundary markers, Windows-safe build
  commits, pre-render limits, watcher invalidation, and starter generation

## v1.0.8 (2026-07-10)

### Performance and Build

- Parallelized build-time prerendering for CSR, SSG, ISR, and PPR routes while preserving manifest
  order
- Reused the configured build parallelism for prerender work to reduce production build time
- Kept client bundling parallelism capped to available work to avoid oversubscription
- Reduced the demo production build benchmark from about 2.3s to about 1.1s

### Styling

- Collected CSS through the application dependency graph, including styles imported from outside
  `app/` and nested local CSS `@import` dependencies
- Added project-relative `css.entries` for unimported global style files and directories
- Preserved runtime CSS-in-JS style objects and `<style>` elements, with external style HMR and
  production-copy coverage
- Added actionable diagnostics for unresolved styles, unsafe entries, and preprocessors without a
  transform plugin

## v1.0.7 (2026-07-10)

### Performance and Bundling

- Reused one persistent Node worker for JavaScript config plugin hooks during each build
- Forwarded plugin transform Source Map v3 data into generated client bundle maps
- Added route-scoped shared chunk metadata and `modulepreload` hints to runtime and pre-rendered
  HTML
- Ensured pre-rendered SSG, ISR, PPR, and CSR output loads hashed hydration assets from the client
  manifest
- Added fixture-driven advanced TypeScript/JSX parser coverage and fixed multiline enums,
  `implements`, and namespaced JSX tags
- Invalidated native compile artifacts when imported config/plugin dependencies change
- Added shared build-cache directories via `cache.dir` or `RUVYXA_BUILD_CACHE_DIR`
- Pre-bundled dev route dependencies in background across every persistent Node worker
- Added consistent client directory and chunk-manifest references to every deployment adapter

## v1.0.6 (2026-07-09)

### Highlights

- SSG, ISR, and PPR pre-rendering support added to the rendering pipeline
- New runtime SSG renderer for server-side page pre-rendering at build time
- CSR minimal shell HTML generation for client-side rendered pages
- Revalidation metadata tracking for ISR routes
- Dev server and build output updated with prerendered routes manifest
- Demo examples demonstrating SSG, ISR, PPR, and CSR rendering strategies
- Codebase-wide formatting standardization with Prettier configuration
- `render_api` refactored to use structured request object for improved maintainability
- Documentation overhaul across all guides (rendering strategies, cache, security, middleware)
- pnpm requirement upgraded from 10+ to 11+

### Rust Crates

- **ruvyxa_cli**:
  - SSG/ISR/PPR pre-rendering at build time with dynamic route support
  - `getStaticParams` resolution for dynamic routes during build
  - Build output includes prerendered routes manifest and prerender stats
  - Code formatting improvements
- **ruvyxa_dev_server**:
  - Prerender directory support in dev server and production configs
  - `render_api` refactored to accept `RenderApiRequest` struct instead of multiple params
  - Reduced parameter passing complexity and improved type safety
  - Worker pool and router enhancements
- **ruvyxa_graph**:
  - Route manifest generation updates for prerendering
  - Enhanced route discovery

### npm Packages

- All packages updated with version bumps
- **@ruvyxa/core**: Added `RenderStrategy` enum and rendering configuration to types
- **ruvyxa/runtime**:
  - New `ssg-renderer.mjs` for server-side page rendering
  - `worker-pool.mjs` modernized with improved concurrent request handling
  - All runtime modules formatted to new Prettier standards
- All adapter packages updated with `tsconfig.check.json` and formatting
- All CLI binary packages updated

### Examples

- **demo**:
  - New SSG blog with `[slug]` dynamic routes (`app/ssg-blog/`)
  - New ISR page with revalidation (`app/isr-page/`)
  - New PPR page with partial pre-rendering (`app/ppr-page/`)
  - New CSR page with client-side rendering (`app/static-page/`)
  - Static page example
  - Updated layout, routing, and configurations

### Documentation

- Updated README with rendering strategies, pnpm 11+ requirement, expanded crate descriptions
- Updated CONTRIBUTING with correct Rust verification flags and adapter guidelines
- Enhanced `docs/architecture/project-structure.md` with crate capabilities and features
- Updated `docs/routing.md`, `docs/data.md`, `docs/actions.md` with rendering strategy details
- Revamped `docs/debugging.md`, `docs/deployment.md`, `docs/performance.md`
- Expanded `docs/production-readiness.md` with cache configuration and security
- Improved `docs/publishing.md` and `docs/parity.md`
- `docs/testing.md` updated with API renderer test documentation

### Infrastructure

- Added `.prettierrc` and `.prettierignore` for consistent code formatting
- pnpm requirement changed from `^10.32.1` to `^11.7.0`
- Package metadata validation uses dynamic license from root `package.json`
- All `package.json` files updated with version and dependency sync
- TypeScript config check files added to adapter packages
- GitHub Actions workflows updated for formatting consistency

### Testing

- New `api-renderer.test.mjs` test suite for API rendering
- Updated `compiler.test.mjs`, `client-renderer.test.mjs`, `action-renderer.test.mjs`
- Updated adapter tests for all 6 deployment targets
- Updated core config and server tests
- `test-full-flow.ps1` updated with expanded coverage

## v1.0.5 (2026-07-09)

### Highlights

- Full Ruvyxa Bundler pipeline with AST parsing, plugin system, chunking, and tree-shaking
- New `demo` example app replacing `basic-app`
- Comprehensive end-to-end test script (`test-full-flow.ps1`)
- Project structure and engineering backlog documentation
- README logo switched to local asset for reliability

### Rust Crates

- **ruvyxa_bundler**: Major feature expansion
  - AST module (`ast.rs`) for structured parsing of imports, exports, JSX, decorators, TypeScript
  - Plugin system (`plugin.rs`) for custom transformations in the bundler pipeline
  - Chunking module (`chunking.rs`) for dynamic import split points and output chunk generation
  - Context module (`context.rs`) for shared bundler execution state across parallel workers
  - Types module (`types.rs`) with core bundler type definitions
  - Tree-shaking as separate step before minification (`treeShake` build option)
  - Cache hit tracking via `cache_hit` field on `CompiledModule`
  - Plugin-runner module for runtime plugin execution
  - Resolver enhancements: CommonJS `require()`, dynamic `import()`, improved caching
  - Source map improvements
  - Parallel cache reuse across bundle jobs
- **ruvyxa_cli**: Integrated new bundler components, expanded CLI commands
- **ruvyxa_dev_server**: Render cache improvements, HMR tracker updates, worker pool enhancements
- **ruvyxa_middleware**: WASM plugin system improvements
- **ruvyxa_graph**: Graph module updates
- **ruvyxa_diagnostics**: Diagnostic enhancements

### npm Packages

- All packages bumped to v1.0.5
- **@ruvyxa/core**: Added `utils.ts`, `PluginContext` and `TransformResult` exports, config updates
- **@ruvyxa/react**: Package updates
- **ruvyxa/runtime**: Added `plugin-runner.mjs`, `config-renderer.mjs` enhancements, `compiler.mjs`
  updates
- **adapters**: All 6 adapter packages updated with platform info and README improvements
- **CLI platform binaries**: All 5 platform packages updated
- **create-ruvyxa**: Updates

### Examples

- Replaced `basic-app` with comprehensive `demo` example
  - Multiple route patterns (static, dynamic `[slug]`, catchall `[...slug]`)
  - Todos with server actions and in-memory DB
  - Blog routes, environment variables page
  - Full TypeScript + Tailwind CSS setup
  - AGENTS.md and CLAUDE.md for AI-assisted development

### Documentation

- Added `docs/architecture/project-structure.md`
- Added `docs/roadmap/engineering-backlog.md`
- Updated debugging, deployment, parity, performance, production-readiness docs
- Bundler comparison documentation (`bundler-comparison.md`)
- README refreshed with new logo, features, and bundler comparison link

### Testing

- New `scripts/test-full-flow.ps1` end-to-end test script
- Expanded compiler test coverage
- Integration tests for all adapter packages
- AST parsing tests across import forms

### Infrastructure

- Removed obsolete `basic-app` example
- Cleaned up old design spec documents
- CLAUDE.md and AGENTS.md updated

---

## v1.0.4 (2026-07-09)

### Highlights

- `ruvyxa check` command for pre-deploy verification
- Type checking, build validation, dev/prod parity, and page smoke rendering
- Plugin contract documentation and type exports
- Simplified template structure (removed `.env.example`, consolidated CSS)

### Rust Crates

- **ruvyxa_cli**: Added `CheckArgs` and `ruvyxa check` command
  - Runs type checking, build validation, dev/prod parity, smoke rendering
- **ruvyxa_dev_server**: Exported `render_request` for programmatic rendering in `check` command
- **ruvyxa_graph**: Route discovery and manifest generation updates
- All crates bumped to v1.0.4

### npm Packages

- All packages bumped to v1.0.4
- **@ruvyxa/core**:
  - Exported `PluginContext` and `TransformResult` types
  - Added comprehensive Plugin Contract section to README
  - New `types.ts` for shared type definitions
  - `config.ts` improvements
- **ruvyxa**: Type exports synced, removed unused CSS module declarations
- **create-ruvyxa**: Package manager detection utility
- All adapters and CLI binaries updated

### Examples & Templates

- **basic-app**: Removed `.env.example`, simplified
- **minimal template**:
  - Removed todos example and about page
  - Consolidated `global.css` → `globals.css`
  - Removed legacy API examples
  - Updated TypeScript configuration

### Documentation

- Plugin Contract documentation with example implementation
- `check` positioned as primary verification command before `build`
- SKILL.md and app-guide.md updated to recommend `pnpm check`
- `test:parity` command description clarified
- Updated routing, data, debugging, deployment, getting-started docs
- Removed outdated `Plan.md`

### Testing

- `core/config.test.ts` test suite for configuration validation
- Expanded compiler test coverage
- create-ruvyxa test suite enhancements
- Verification guidance: use `check` for routine changes, `parity`/`analyze` for drill-down

---

## v1.0.3 (2026-07-08)

### Highlights

- Incremental caching and HMR tracking for bundler and dev server
- Centralized test directory structure (`tests/`)
- First-class adapter packages with dedicated tests
- React utilities: error boundary, hydration, `useLoader` hook

### Rust Crates

- **ruvyxa_bundler**:
  - Incremental graph cache with `incremental.rs` (file fingerprinting via blake3)
  - `CachedModuleEntry` and `GraphManifest` for persistent dependency storage
  - Fast-reject optimization (mtime/size check before hashing)
  - Cache hit tracking improvements
  - Compiler enhancements: TypeScript annotation stripping fixes, JSX child detection
  - All crates bumped to v1.0.3
- **ruvyxa_cli**: Path validation in `ProjectConfig`, command additions
- **ruvyxa_dev_server**:
  - HMR tracker module (`hmr_tracker.rs`)
  - Module invalidation tracking and dependency relationship mapping
  - Render cache expiry and validation improvements
  - Worker pool task scheduling and error handling enhancements
- **ruvyxa_middleware**: Updates
- **ruvyxa_graph**: Updates
- **ruvyxa_diagnostics**: Updates

### npm Packages

- All packages bumped to v1.0.3
- **@ruvyxa/react**:
  - Error boundary component
  - Hydration utilities for React client initialization
  - `useLoader` hook for data loading patterns
- **@ruvyxa/core**: Server implementation improvements
- **ruvyxa/runtime**:
  - Full runtime compiler (`compiler.mjs`)
  - SSR renderer, action renderer, client renderer, API renderer improvements
  - Worker pool enhancements
- **create-ruvyxa**: Package manager detection utility
- All adapters and CLI binaries updated

### Testing

- Reorganized tests from package-local to centralized `tests/` directory
- Dedicated test files for each adapter: Bun, Cloudflare, Netlify, Node, Static, Vercel
- `tests/packages/core/server.test.ts`
- `tests/packages/ruvyxa/compiler.test.mjs`
- `tests/packages/ruvyxa/action-renderer.test.mjs`
- `tests/packages/ruvyxa/client-renderer.test.mjs`
- vitest configuration
- Adapter test coverage for all deployment targets

### Documentation

- `docs/testing.md` with testing layout guidance
- Updated debugging, performance, production-readiness, publishing docs
- README logo enlarged
- CLI platform binary READMEs

### Infrastructure

- Dashmap, memmap2, parking_lot dependencies for concurrent caching
- Clap bumped to 4.6
- Dependency updates

---

## v1.0.2 (2026-06-18)

### Highlights

- First release of `ruvyxa_bundler` — Ruvyxa Bundler
- `ruvyxa_middleware` crate with WASM plugin support
- Compression, caching, and worker pool in dev server
- Upgraded toolchain: Node.js 22, Rust 1.96, pnpm 10

### Rust Crates

- **ruvyxa_bundler** (new crate):
  - Ruvyxa Bundler TypeScript/JSX compiler pipeline
  - Boundary checker for server/client module isolation
  - Caching layer with blake3 hashing for incremental builds
  - AST transformation and code generation
  - Linker for module resolution and bundle generation
  - Minifier for production bundle optimization
  - Source map generation
  - Module path resolution and dependency tracking
  - Output formatter with bundle metadata
- **ruvyxa_middleware** (new crate):
  - Builtin middleware implementations
  - Config-driven middleware stack
  - WASM plugin system for custom middleware
- **ruvyxa_dev_server**:
  - HTTP compression (gzip + brotli) via tower-http
  - Render cache system with blake3 hashing
  - Radix router for efficient route matching
  - Node.js worker pool for concurrent request handling
- **ruvyxa_cli**: Integrated bundler, middleware, expanded CLI
- All crates bumped to v1.0.2

### npm Packages

- All packages bumped to v1.0.2
- **@ruvyxa/core**: Server refactoring, config improvements, type safety
- **@ruvyxa/react**: Package initialized with exports
- **ruvyxa/runtime**:
  - `config-renderer.mjs` for runtime configuration
  - `worker-pool.mjs` for Node.js worker management
- **CLI platform binaries**: All 5 platform packages with native binaries
- **create-ruvyxa**: Enhanced CLI with platform detection
- **@ruvyxa/adapter-***: All 6 adapters with README and teamplate support

### CI/CD

- GitHub Actions CI and release workflows
- Cargo caching (Swatinem/rust-cache@v2)
- Smart publishing via `publish-if-new.mjs`
- Version resolution and tag management
- Cross-platform npm pack smoke testing
- Exponential backoff retry for cargo fetch and pnpm install
- `bump-version.mjs` script
- `--locked` flag for reproducible builds

### Toolchain

- Node.js upgraded from 20 to 22
- Rust toolchain upgraded from 1.80+ to 1.96+
- pnpm upgraded to v10
- TypeScript 6.0
- pnpm action v5

### Templates

- **minimal template**: Complete Ruvyxa starter with routing, API, todos, Tailwind CSS
- AGENTS.md and CLAUDE.md for AI-assisted development
- `config-renderer.mjs` integration
- TypeScript configuration with path aliases

### Documentation

- Comprehensive README with hero section, features, CLI commands
- Full CONTRIBUTING.md with development setup, conventions, workflows
- Getting started, routing, data, actions, debugging, deployment, performance docs
- Production readiness and publishing guides
- Parity testing documentation
- Security documentation in SECURITY.md
- Skill framework documentation (SKILL.md, app-guide.md)

---

## v1.0.1 (2026-06-17)

### Highlights

- Cross-platform npm pack smoke testing
- HMR WebSocket payload optimization
- `create-ruvyxa` CLI UX improvements
- CI/CD reliability and version management

### Rust Crates

- All crates bumped to v1.0.1 (ruvyxa_cli, ruvyxa_dev_server, ruvyxa_diagnostics, ruvyxa_graph)
- **ruvyxa_dev_server**: Simplified HMR WebSocket handling using pre-serialized messages from
  channel

### npm Packages

- All packages bumped to v1.0.1
- **create-ruvyxa**:
  - Try-catch error handling with graceful error display
  - Formatted next steps after app creation (cd, pnpm install, pnpm dev)
  - Target directory validation (exists + empty check)
  - Clear error messages for non-empty directories
- **ruvyxa**: Release packaging scripts
- **@ruvyxa/adapter-***: All adapters updated
- **CLI platform binaries**: All platform packages updated

### CI/CD

- `resolve-version` job for version extraction and git tag validation
- Auto tag creation on `workflow_dispatch`
- Git tag existence check via `git ls-remote`
- Release summary in GitHub step summary
- `release:bump` script for syncing workspace versions
- Cross-platform npm pack smoke detection (dynamic tarball resolution)
- macOS 13 build target removal
- HMR error handling simplification

### Documentation

- `docs/publishing.md` with npm publishing guidelines
- Updated deployment docs
- Version reference updates across docs
- README version badges

### Infrastructure

- `scripts/validate-package-metadata.mjs`
- `scripts/pack-smoke.mjs` with dynamic tarball detection
- Platform-specific native binary preparation scripts

---

## v1.0.0 (2026-06-17)

### Highlights

- Initial production release of Ruvyxa
- Native Rust CLI with full-stack React framework
- Built-in development server and production server
- Route discovery and manifest generation
- Diagnostic system with error codes

### Rust Crates

- **ruvyxa_cli**:
  - CLI entry point with commands: `dev`, `build`, `start`, `preview`, `routes`, `analyze`,
    `doctor`, `clean`, `trace`, `bench`, `test:parity`
  - Project configuration and build pipeline
  - PID file management
- **ruvyxa_dev_server**:
  - Development server with HMR and WebSocket support
  - Production server with static file serving
  - Node.js runtime integration
- **ruvyxa_diagnostics**:
  - Diagnostic type system: warnings, errors, hints, tips
  - Structured diagnostic output
- **ruvyxa_graph**:
  - Route discovery from file system
  - Route manifest generation
  - Layout and page tree construction

### npm Packages

- **ruvyxa**: Main CLI wrapper package with native binary resolution
- **@ruvyxa/core**: Core framework with server, config, types, request/response handling
- **@ruvyxa/react**: React integration with SSR support
- **create-ruvyxa**: Project scaffolding CLI
- **@ruvyxa/adapter-bun**: Bun deployment adapter
- **@ruvyxa/adapter-cloudflare**: Cloudflare Workers deployment adapter
- **@ruvyxa/adapter-netlify**: Netlify deployment adapter
- **@ruvyxa/adapter-node**: Node.js deployment adapter
- **@ruvyxa/adapter-static**: Static site generation adapter
- **@ruvyxa/adapter-vercel**: Vercel deployment adapter
- **@ruvyxa/cli-darwin-arm64**: macOS ARM64 native binary
- **@ruvyxa/cli-linux-arm64**: Linux ARM64 native binary
- **@ruvyxa/cli-linux-x64**: Linux x64 native binary
- **@ruvyxa/cli-win32-x64**: Windows x64 native binary

### Runtime

- `ssr-renderer.mjs` — Server-side rendering
- `client-renderer.mjs` — Client hydration and rendering
- `action-renderer.mjs` — Server action handling
- `api-renderer.mjs` — API route handling
- `config-renderer.mjs` — Runtime configuration
- `worker-pool.mjs` — Worker pool management

### Examples

- **basic-app**: Starter application with:
  - Layout and page routing
  - About page
  - Blog with dynamic `[slug]` routes
  - Todos with server actions
  - Tailwind CSS styling
  - TypeScript configuration

### Templates

- **minimal template**: Minimal Ruvyxa starter
  - Single page with layout
  - Basic route structure
  - TypeScript + Tailwind CSS

### Documentation

- README.md with feature overview, getting started, examples
- CLI command documentation
- Architecture overview

### Infrastructure

- Rust workspace with 5 crates
- pnpm monorepo with 18 packages
- GitHub repository setup
- Prebuilt native CLI binaries for 5 platforms (Windows x64/ARM64, macOS ARM64, Linux x64/ARM64)
- npm publishing configuration
- TypeScript base configuration

---

## Pre-release History (unversioned)

The following commits occurred before the v1.0.0 tag and represent the initial project bootstrap:

| Date       | Description                                                                   |
| ---------- | ----------------------------------------------------------------------------- |
| 2026-06-17 | Initial project scaffold (`first commit`)                                     |
|            | Application structure with Tailwind CSS, todos page, navigation               |
|            | Security headers, performance benchmarks, deployment docs                     |
|            | Repository references updated, npm publishing documentation                   |
|            | Prebuilt native CLI binaries for multiple platforms                           |
|            | Adapter packages initialized (Bun, Cloudflare, Netlify, Node, Static, Vercel) |
|            | Foundational documentation and contributing guide                             |

---

## Release Tags

| Tag       | Date       | Type       |
| --------- | ---------- | ---------- |
| `v1.0.0`  | 2026-06-17 | Production |
| `v1.0.1`  | 2026-06-17 | Patch      |
| `v1.0.2`  | 2026-06-18 | Minor      |
| `v1.0.3`  | 2026-07-08 | Minor      |
| `v1.0.4`  | 2026-07-09 | Minor      |
| `v1.0.5`  | 2026-07-09 | Minor      |
| `v1.0.6`  | 2026-07-09 | Patch      |
| `v1.0.7`  | 2026-07-10 | Minor      |
| `v1.0.8`  | 2026-07-10 | Minor      |
| `v1.0.9`  | 2026-07-10 | Patch      |
| `v1.0.10` | 2026-07-11 | Minor      |
| `v1.0.11` | 2026-07-12 | Minor      |
| `v1.0.12` | 2026-07-13 | Minor      |
| `v1.0.13` | 2026-07-14 | Patch      |
| `v1.0.14` | 2026-07-16 | Minor      |
| `v1.0.15` | 2026-07-18 | Minor      |
| `v1.0.16` | 2026-07-20 | Minor      |
| `v1.0.17` | 2026-07-22 | Minor      |
| `v1.0.18` | 2026-07-22 | Patch      |
| `v1.0.19` | 2026-07-23 | Patch      |
| `v1.0.20` | 2026-07-24 | Minor      |
| `v1.0.21` | 2026-07-24 | Patch      |
