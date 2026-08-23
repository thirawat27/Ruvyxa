import { spawn } from 'node:child_process'
import { createReadStream, existsSync, readFileSync, statSync } from 'node:fs'
import { createServer, request as httpRequest } from 'node:http'
import path from 'node:path'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { pathToFileURL } from 'node:url'
import { gunzipSync } from 'node:zlib'

const RUNTIMES = ['node', 'bun', 'deno', 'edge']
const [runtime, deploymentDirectory, portArg, assetsArg] = process.argv.slice(2)
if (!RUNTIMES.includes(runtime) || !deploymentDirectory || !portArg) {
  console.error(
    'usage: node scripts/smoke-runtime-adapter.mjs <node|bun|deno|edge> <deployment-dir> <port> [assets-dir]',
  )
  console.error(
    '  edge takes the worker directory; assets default to <deployment-dir>/../assets, which is',
  )
  console.error('  where the cloudflare adapter puts the files its platform serves.')
  process.exit(2)
}

const port = Number(portArg)
const base = `http://127.0.0.1:${port}`

/**
 * The route strategies this deployment can serve.
 *
 * An edge target has no ISR — `adapter.supports` says so and the build refuses
 * such a route — so the fixture it is built from has none either, and the check
 * for it has nothing to ask. Skipped checks are printed rather than dropped: a
 * suite that quietly shrinks reads as one that passed.
 */
const capabilities = runtime === 'edge' ? new Set(['ssr', 'ssg', 'csr', 'api']) : null

let output = ''
let child = null
let server = null

if (runtime === 'edge') await startEdgeWorker()
else startRuntimeProcess()

/** Spawn the emitted standalone server, which is a program on these runtimes. */
function startRuntimeProcess() {
  const entry = path.join('server', 'index.mjs')
  const args = runtime === 'deno' ? ['run', '-A', '--no-prompt', entry] : [entry]
  child = spawn(runtimeExecutable(runtime), args, {
    cwd: path.resolve(deploymentDirectory),
    env: { ...process.env, HOST: '127.0.0.1', PORT: String(port) },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  child.stdout.on('data', (chunk) => (output += chunk))
  child.stderr.on('data', (chunk) => (output += chunk))
}

/**
 * Put a real HTTP server in front of the emitted worker.
 *
 * An edge adapter emits a *module*, not a program: Cloudflare imports it and
 * calls `fetch`, and serves the asset directory beside it from its own network
 * before the worker is ever invoked. Nothing here could exercise that half, so
 * the whole edge lane — the only one whose bundles have no `process` at all —
 * was checked by reading the files it wrote rather than by asking it anything.
 * A `NODE_ENV` that stayed `"development"` in every worker's SSR pass survived
 * that way.
 *
 * Static first, then the worker, because that is the order the platform routes
 * in and the reason a worker never sees a request for a hashed bundle.
 */
async function startEdgeWorker() {
  const workerDir = path.resolve(deploymentDirectory)
  const assetsDir = path.resolve(assetsArg ?? path.join(workerDir, '..', 'assets'))
  const module = await import(pathToFileURL(path.join(workerDir, 'index.mjs')).href)
  const worker = module.default
  if (typeof worker?.fetch !== 'function') {
    throw new Error(`${workerDir}/index.mjs does not export a { fetch } worker`)
  }

  server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, base)
      const file =
        request.method === 'GET' || request.method === 'HEAD'
          ? staticFile(assetsDir, url.pathname)
          : null
      if (file) {
        response.statusCode = 200
        response.setHeader('content-type', assetContentType(file))
        for (const [name, value] of assetHeaders(url.pathname)) response.setHeader(name, value)
        createReadStream(file).pipe(response)
        return
      }
      await answerWithWorker(worker, request, response, url)
    } catch (error) {
      output += `${error?.stack ?? error}\n`
      response.statusCode = 500
      response.end('worker threw')
    }
  })
  await new Promise((resolve) => server.listen(port, '127.0.0.1', resolve))
}

/** Turn a Node request into a `Request`, call the worker, and write the answer back. */
async function answerWithWorker(worker, request, response, url) {
  const chunks = []
  for await (const chunk of request) chunks.push(chunk)
  const body = chunks.length > 0 ? Buffer.concat(chunks) : undefined
  const answer = await worker.fetch(
    new Request(url.toString(), {
      method: request.method,
      headers: Object.entries(request.headers).flatMap(([key, value]) =>
        Array.isArray(value) ? value.map((item) => [key, item]) : [[key, String(value)]],
      ),
      body,
      duplex: body ? 'half' : undefined,
    }),
    {},
    { waitUntil() {} },
  )
  response.statusCode = answer.status
  for (const [key, value] of answer.headers.entries()) {
    if (key !== 'set-cookie') response.setHeader(key, value)
  }
  const cookies = answer.headers.getSetCookie?.() ?? []
  if (cookies.length > 0) response.setHeader('set-cookie', cookies)
  if (!answer.body) {
    response.end()
    return
  }
  await pipeline(Readable.fromWeb(answer.body), response)
}

/**
 * The `_headers` rules the platform applies to a published file.
 *
 * Workers static assets read this file out of the asset directory and attach
 * what it says; the worker never sees the request. So the cache policy and the
 * security defaults on an edge deployment are a *file the adapter wrote*, not
 * headers any code sets, and asserting them means reading that file the way the
 * platform does. Ignoring it here would have made the asset check pass or fail
 * on this script instead of on the artifact.
 *
 * The format is one path pattern per unindented line, its headers indented
 * under it, `*` matching any run of characters. Later matches win, which is the
 * order the adapter writes them in — `/*` first, then the specific ones.
 */
function assetHeaders(pathname) {
  headerRules ??= parseHeaderRules()
  const matched = new Map()
  for (const rule of headerRules) {
    if (!rule.pattern.test(pathname)) continue
    for (const [name, value] of rule.headers) matched.set(name, value)
  }
  return matched
}

let headerRules = null

function parseHeaderRules() {
  const file = path.join(
    path.resolve(assetsArg ?? path.join(deploymentDirectory, '..', 'assets')),
    '_headers',
  )
  if (!existsSync(file)) return []
  const rules = []
  for (const line of readFileSync(file, 'utf8').split('\n')) {
    if (line.trim() === '') continue
    if (!/^\s/.test(line)) {
      const escaped = line
        .trim()
        .replace(/[.+?^${}()|[\]\\]/g, '\\$&')
        .replaceAll('*', '.*')
      rules.push({ pattern: new RegExp(`^${escaped}$`), headers: [] })
      continue
    }
    const separator = line.indexOf(':')
    if (separator === -1 || rules.length === 0) continue
    rules
      .at(-1)
      .headers.push([
        line.slice(0, separator).trim().toLowerCase(),
        line.slice(separator + 1).trim(),
      ])
  }
  return rules
}

/** The published file a path names, if the asset directory has one. */
function staticFile(root, pathname) {
  const decoded = decodeURIComponent(pathname)
  if (decoded.includes('..')) return null
  const direct = path.join(root, decoded)
  for (const candidate of [direct, path.join(direct, 'index.html')]) {
    if (existsSync(candidate) && statSync(candidate).isFile()) return candidate
  }
  return null
}

function assetContentType(file) {
  const types = {
    '.html': 'text/html; charset=utf-8',
    '.js': 'text/javascript; charset=utf-8',
    '.mjs': 'text/javascript; charset=utf-8',
    '.json': 'application/json; charset=utf-8',
    '.svg': 'image/svg+xml',
    '.txt': 'text/plain; charset=utf-8',
    '.css': 'text/css; charset=utf-8',
  }
  return types[path.extname(file)] ?? 'application/octet-stream'
}

/**
 * What the emitted server has to get right, checked against the real runtime.
 *
 * One endpoint was not enough. The three transports differ in how a request
 * reaches the handler and how a file becomes a response body, so the cases
 * below are chosen for the decisions that differ: the publish directory, the
 * generated route registry, the pre-rendered read, and the headers the shared
 * core attaches. A Bun range bug that served a whole file for a sliced
 * `BunFile` got through a health check without a mark on it.
 */
const checks = [
  {
    name: 'GET /api/health reaches the generated route registry',
    path: '/api/health',
    assert: (response, body) => {
      if (response.status !== 200) return `status ${response.status}`
      if (!body.includes('Ruvyxa')) return `body ${body.slice(0, 120)}`
      return null
    },
  },
  {
    name: 'GET / serves the pre-rendered page',
    path: '/',
    assert: (response, body) => {
      if (response.status !== 200) return `status ${response.status}`
      if (!body.includes('data-smoke="page"')) return `body ${body.slice(0, 120)}`
      return null
    },
  },
  {
    name: 'GET /cached serves an ISR route',
    requires: 'isr',
    path: '/cached',
    assert: (response, body) => {
      if (response.status !== 200) return `status ${response.status}`
      if (!body.includes('data-smoke="cached"')) return `body ${body.slice(0, 120)}`
      return null
    },
  },
  {
    name: 'GET /smoke.svg serves a public asset with its cache policy',
    path: '/smoke.svg',
    assert: (response) => {
      if (response.status !== 200) return `status ${response.status}`
      const type = response.headers.get('content-type')
      if (type !== 'image/svg+xml') return `content-type ${type}`
      const cache = response.headers.get('cache-control')
      if (cache !== 'public, max-age=3600, must-revalidate') return `cache-control ${cache}`
      return null
    },
  },
  {
    name: 'a served page carries the security defaults',
    path: '/',
    assert: (response) => {
      const nosniff = response.headers.get('x-content-type-options')
      if (nosniff !== 'nosniff') return `x-content-type-options ${nosniff}`
      const frame = response.headers.get('x-frame-options')
      if (frame !== 'DENY') return `x-frame-options ${frame}`
      return null
    },
  },
  {
    name: 'an unknown path is a 404 rather than a crash',
    path: '/definitely-not-a-route',
    assert: (response) => (response.status === 404 ? null : `status ${response.status}`),
  },
  {
    // A dynamic server-components route is rendered by the emitted function on
    // every request, so this is the only check that exercises what a deployed
    // build has to carry for itself: the `react-server` graph, the SSR registry
    // that turns a reference id back into a component, and the payload data
    // block the browser hydrates from. Every adapter refused this shape with
    // RUV2213 until the route registry learned to render through that pipeline.
    name: 'GET /rsc renders a dynamic server-components route',
    path: '/rsc',
    assert: (response, body) => {
      if (response.status !== 200) return `status ${response.status}`
      if (!body.includes('data-smoke="rsc"')) return `body ${body.slice(0, 160)}`
      // The client component rendered by the SSR pass. A deployed bundle with
      // more than one React copy throws on its first hook instead, and the
      // route answers 500 rather than markup.
      if (!body.includes('data-smoke="counter"')) return 'the client component did not render'
      // `count <!-- -->0`: React writes a comment between adjacent text nodes
      // so hydration can tell where one ends and the next begins. Its presence
      // is itself evidence the markup came from a real render rather than a
      // string, so the check tolerates it rather than stripping it.
      if (!/count (?:<!-- -->)?0/.test(body)) {
        return 'the client component rendered without its state'
      }
      // Without the payload the document is server-rendered and inert: the
      // browser entry declines to hydrate nothing.
      if (!body.includes('id="__ruvyxa-rsc"')) return 'no Flight payload in the document'
      if (!body.includes('id="__ruvyxa-bootstrap"')) return 'no bootstrap block in the document'
      if (!/<script type="module" src="\/__ruvyxa\/client\//.test(body)) {
        return 'no browser bundle in the document'
      }
      // The half no other assertion here can reach: which *build* of React
      // rendered the payload. A deployment's browser bundle is compiled by the
      // Rust bundler, which folds `NODE_ENV` to production and cannot be told
      // otherwise, while the server half used to read the ambient value — and
      // nothing in an emitted deployment sets one, so `node server/index.mjs`
      // served a development payload to a production client. Every check above
      // passed: the status was 200, the markup was right, the payload was
      // there. The browser threw `Failed to read a RSC payload created by a
      // development version of React` and showed a blank page.
      //
      // Development React writes an owner stack for every row, each frame
      // naming an absolute path on the build machine, which makes the leak
      // itself the cheapest thing to assert — and worth asserting on its own,
      // since a document that publishes `file:///…/home/<user>/…` to every
      // visitor is a defect whatever React does with it.
      if (body.includes('file:///')) {
        return 'the payload carries development React debug frames (build paths leaked)'
      }
      return null
    },
  },
]

/**
 * Compression, checked over a raw request rather than through `fetch`.
 *
 * `fetch` decodes the body and removes `content-encoding` before anything here
 * can look at it, so a server that compressed nothing would be indistinguishable
 * from one that compressed everything — which is how this went unnoticed while
 * the standalone server sent every document and bundle uncompressed.
 */
async function checkCompression() {
  const compressed = await rawGet('/', { 'accept-encoding': 'gzip' })
  if (compressed.headers['content-encoding'] !== 'gzip') {
    throw new Error(
      `${runtime}: a gzip-capable client got content-encoding ` +
        `${compressed.headers['content-encoding']}\n${output}`,
    )
  }
  const vary = String(compressed.headers['vary'] ?? '').toLowerCase()
  if (!vary.includes('accept-encoding')) {
    throw new Error(`${runtime}: compressed response is missing Vary: Accept-Encoding\n${output}`)
  }
  if (compressed.headers['content-length'] !== undefined) {
    throw new Error(`${runtime}: a compressed response kept its identity content-length\n${output}`)
  }
  const decoded = gunzipSync(compressed.body).toString('utf8')
  if (!decoded.includes('data-smoke="page"')) {
    throw new Error(`${runtime}: gunzipped body ${decoded.slice(0, 120)}\n${output}`)
  }
  if (compressed.body.length >= decoded.length) {
    throw new Error(
      `${runtime}: gzip made the page larger (${compressed.body.length} >= ${decoded.length})\n${output}`,
    )
  }
  console.log(`[ok] ${runtime} · GET / is gzipped for a client that accepts it`)

  // A client that refuses every coding must still be served, uncompressed.
  const identity = await rawGet('/', { 'accept-encoding': 'identity' })
  if (identity.headers['content-encoding'] !== undefined) {
    throw new Error(
      `${runtime}: identity-only client got content-encoding ` +
        `${identity.headers['content-encoding']}\n${output}`,
    )
  }
  if (!identity.body.toString('utf8').includes('data-smoke="page"')) {
    throw new Error(`${runtime}: identity body ${identity.body.toString('utf8').slice(0, 120)}`)
  }
  console.log(`[ok] ${runtime} · GET / stays identity for a client that refuses gzip`)
}

/**
 * The payload endpoint a soft navigation into a server-components route calls.
 *
 * Reported 501 in every deployed build until the route registry could render
 * through the server-components pipeline, so such a navigation fell back to a
 * full document load. Checked here rather than in a unit test because the
 * contract is the *response*: the browser router calls one endpoint and must
 * not be able to tell which host answered it.
 */
async function checkRscPayload() {
  const payload = await rawGet('/__ruvyxa/rsc?path=/rsc', { 'x-ruvyxa-rsc': '1' })
  if (payload.status !== 200) {
    throw new Error(`${runtime}: payload endpoint answered ${payload.status}\n${output}`)
  }
  if (!String(payload.headers['content-type']).startsWith('text/x-component')) {
    throw new Error(`${runtime}: payload content-type ${payload.headers['content-type']}`)
  }
  // Flight is line-delimited `<id>:<value>` rows. Which id arrives first is
  // completion order, not document order, so the check is the shape and the
  // client reference the page actually has — not a fixed first row.
  const body = payload.body.toString('utf8')
  if (!/(^|\n)\d+:/.test(body)) {
    throw new Error(`${runtime}: payload is not Flight rows: ${body.slice(0, 200)}\n${output}`)
  }
  if (!body.includes('ruv:')) {
    throw new Error(`${runtime}: payload names no client reference: ${body.slice(0, 200)}`)
  }

  // The header is what keeps this endpoint out of reach of a cross-origin page,
  // which cannot set it without a preflight.
  const unguarded = await rawGet('/__ruvyxa/rsc?path=/rsc', {})
  if (unguarded.status !== 400) {
    throw new Error(`${runtime}: payload endpoint answered ${unguarded.status} without its header`)
  }
  console.log(`[ok] ${runtime} · /__ruvyxa/rsc serves a payload and refuses an unmarked request`)
}

/**
 * The other verb on the same path: running one of the route's server functions.
 *
 * `POST /__ruvyxa/rsc` answered `405` in every deployed build, and every check
 * above stayed green while it did — the document rendered, hydrated, and
 * returned 200. What broke was a rejected promise inside the browser: clicking
 * anything wired to a server function threw `Connection closed.` and left a
 * blank page. So this is asked over HTTP, in the shape React asks it.
 *
 * The reference id comes out of the page's own browser bundle rather than being
 * spelled here, because it is derived from the module's path and a fixture that
 * restated it would only prove the two spellings match.
 */
async function checkServerFunction() {
  const document = await rawGet('/rsc', {})
  const bundle = document.body.toString('utf8').match(/src="(\/__ruvyxa\/client\/[^"]+)"/)
  if (!bundle) throw new Error(`${runtime}: the document names no browser bundle`)
  const source = (await rawGet(bundle[1], {})).body.toString('utf8')
  // `'use server'` modules get the `s_` prefix; a client reference gets `m_`.
  // Only the module id is a literal in the bundle — the browser proxy appends
  // the export name at call time — so the export this fixture declares is
  // appended here rather than searched for. The `{16}` is what keeps this off
  // the runtime's own `ruv:s_[a-f0-9]{16}` validation pattern, which is also in
  // the bundle and is not an id.
  const moduleId = source.match(/ruv:s_[0-9a-f]{16}/)
  if (!moduleId) {
    throw new Error(`${runtime}: the browser bundle names no server function`)
  }
  const reference = [`${moduleId[0]}#echo`]

  const called = await rawPost(
    '/__ruvyxa/rsc?path=/rsc',
    {
      'x-ruvyxa-rsc': '1',
      'x-ruvyxa-action': reference[0],
      'content-type': 'text/plain;charset=UTF-8',
      // What `encodeReply` produces for a single string argument.
    },
    '["smoke"]',
  )
  if (called.status !== 200) {
    throw new Error(
      `${runtime}: server function answered ${called.status}: ${called.body.toString('utf8').slice(0, 200)}
${output}`,
    )
  }
  const answer = called.body.toString('utf8')
  if (!answer.includes('server:smoke')) {
    throw new Error(`${runtime}: server function returned ${answer.slice(0, 200)}`)
  }

  const unguarded = await rawPost('/__ruvyxa/rsc?path=/rsc', { 'x-ruvyxa-rsc': '1' }, '["smoke"]')
  if (unguarded.status !== 400) {
    throw new Error(`${runtime}: a call naming no reference answered ${unguarded.status}`)
  }
  console.log(`[ok] ${runtime} · POST /__ruvyxa/rsc runs one of the route's server functions`)
}

/** One GET with exact headers and no transparent decoding. */
function rawGet(pathname, headers) {
  return new Promise((resolve, reject) => {
    const call = httpRequest(
      { host: '127.0.0.1', port, path: pathname, method: 'GET', headers },
      (response) => {
        const chunks = []
        response.on('data', (chunk) => chunks.push(chunk))
        response.on('end', () =>
          resolve({
            status: response.statusCode,
            headers: response.headers,
            body: Buffer.concat(chunks),
          }),
        )
        response.on('error', reject)
      },
    )
    call.on('error', reject)
    call.end()
  })
}

/** One POST with exact headers, for the endpoints that take a body. */
function rawPost(pathname, headers, body) {
  return new Promise((resolve, reject) => {
    const payload = Buffer.from(body, 'utf8')
    const call = httpRequest(
      {
        host: '127.0.0.1',
        port,
        path: pathname,
        method: 'POST',
        headers: { ...headers, 'content-length': String(payload.byteLength) },
      },
      (response) => {
        const chunks = []
        response.on('data', (chunk) => chunks.push(chunk))
        response.on('end', () =>
          resolve({
            status: response.statusCode,
            headers: response.headers,
            body: Buffer.concat(chunks),
          }),
        )
        response.on('error', reject)
      },
    )
    call.on('error', reject)
    call.end(payload)
  })
}

try {
  await waitUntilServing()
  let ran = 0
  for (const check of checks) {
    if (check.requires && capabilities && !capabilities.has(check.requires)) {
      console.log(`[skip] ${runtime} · ${check.name} — this target has no ${check.requires}`)
      continue
    }
    const response = await fetch(base + check.path)
    const body = await response.text()
    const failure = check.assert(response, body)
    if (failure) throw new Error(`${runtime}: ${check.name} — ${failure}\n${output}`)
    console.log(`[ok] ${runtime} · ${check.name}`)
    ran += 1
  }
  // Compression belongs to whatever serves the bytes. The standalone servers do
  // it themselves — `standalone-server.ts` negotiates and encodes — and this is
  // the only place that proves it, since `fetch` decodes transparently. On an
  // edge target the platform's asset network answers a static route and
  // compresses on its own, and the shared handler encodes nothing at all, so
  // there is no claim here to check rather than a check being skipped for
  // convenience.
  if (runtime === 'edge') {
    console.log(`[skip] ${runtime} · content negotiation — the platform owns it on this target`)
  } else {
    await checkCompression()
    ran += 2
  }
  await checkRscPayload()
  await checkServerFunction()
  console.log(`[ok] ${runtime} deployment artifact passed ${ran + 2} checks`)
} finally {
  await stopServing()
}

// Explicit, and only on the `edge` transport. The other three run the artifact
// as a child process, so killing it drains everything it held. This one imports
// the worker into *this* process, and a worker is a module its platform never
// asks to let a process exit — Cloudflare has no process to exit. Waiting for
// the loop to drain would be asking the artifact for a property nothing about
// it promises, and it hung there with every check already reported. A thrown
// check still exits non-zero, above this line.
if (runtime === 'edge') process.exit(0)

/** Stop whichever transport was started, without leaving the port held. */
async function stopServing() {
  if (server) {
    // `closeAllConnections` before `close`, because `fetch` keeps its sockets
    // alive and `close` waits for every one of them. Without it the script
    // finished its checks and then hung with nothing left to do.
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
    return
  }
  child.kill()
  await Promise.race([
    new Promise((resolve) => child.once('exit', resolve)),
    new Promise((resolve) => setTimeout(resolve, 2_000)),
  ])
}

/** Poll until the server answers, so a slow cold start is not read as a failure. */
async function waitUntilServing() {
  const deadline = Date.now() + 15_000
  let lastError
  while (Date.now() < deadline) {
    if (child && child.exitCode !== null) {
      throw new Error(`server exited with ${child.exitCode}: ${output}`)
    }
    try {
      const response = await fetch(`${base}/api/health`)
      await response.arrayBuffer()
      return
    } catch (error) {
      lastError = error
      await new Promise((resolve) => setTimeout(resolve, 200))
    }
  }
  throw new Error(`${runtime} server never answered: ${lastError}\n${output}`)
}

/** Real executables a Windows command shim of `name` could be standing in front of. */
function windowsCandidates(name, directory) {
  if (name === 'node') return [path.join(directory, 'node.exe')]
  if (name === 'bun') {
    return [path.join(directory, 'bun.exe'), path.join(directory, 'node_modules/bun/bin/bun.exe')]
  }
  return [
    path.join(directory, 'deno.exe'),
    path.join(directory, 'node_modules/deno/deno.exe'),
    path.join(directory, 'node_modules/deno/node_modules/@deno/win32-x64/deno.exe'),
  ]
}

function runtimeExecutable(name) {
  if (process.platform !== 'win32') return name
  const pathValue = Object.entries(process.env).find(([key]) => key.toLowerCase() === 'path')?.[1]
  for (const directory of (pathValue ?? '').split(path.delimiter)) {
    const executable = windowsCandidates(name, directory).find(existsSync)
    if (executable) return executable
  }
  throw new Error(`could not resolve the ${name} executable behind its Windows command shim`)
}
