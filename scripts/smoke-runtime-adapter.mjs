import { spawn } from 'node:child_process'
import {
  createReadStream,
  existsSync,
  mkdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import { createServer, request as httpRequest } from 'node:http'
import path from 'node:path'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { pathToFileURL } from 'node:url'
import { gunzipSync } from 'node:zlib'

/**
 * Every transport an adapter can emit, named by the lane that drives it.
 *
 * `node`/`bun`/`deno` and `aws` emit a **program**, so they are spawned. `edge`
 * and `netlify` emit a **fetch module**, so a server is put in front of them.
 * `vercel` and `firebase` emit a **Node request handler**, so the same server
 * calls them with `(req, res)` — which is a third shape, and the one that
 * carries the most platform-specific glue: Vercel's is plain `(req, res, ctx)`
 * and Firebase's is Express-shaped inside `onRequest`.
 *
 * They are separate lanes rather than one "serverless" lane because the glue is
 * exactly what has never been exercised. The four serverless adapters share
 * `createHandler`, which node/bun/deno already prove; what they do not share is
 * how a request reaches it and how a response leaves.
 */
const RUNTIMES = [
  'node',
  'bun',
  'deno',
  'edge',
  'aws',
  'railway',
  'render',
  'vercel',
  'netlify',
  'firebase',
  'static',
]
const SPAWNED = new Set(['node', 'bun', 'deno', 'aws', 'railway', 'render'])
/**
 * Spawned lanes whose program is Node rather than a runtime of the same name.
 *
 * `railway` and `render` emit the same standalone server and the same directory
 * layout as `node`. They are separate lanes anyway, because "the same layout" is
 * a claim about two adapters that can drift apart quietly — and when it breaks,
 * a failure that names the adapter is the whole point.
 */
const NODE_PROGRAM = new Set(['aws', 'railway', 'render'])
const FETCH_MODULE = new Set(['edge', 'netlify'])
const NODE_HANDLER = new Set(['vercel', 'firebase'])
/**
 * Which application's expectations to hold the deployment to.
 *
 * The transports below are the same whatever is deployed; what a check *asks
 * for* is not. `deploy-smoke` is four routes chosen for the emitted server's own
 * decisions, and it is the only fixture every adapter can build. `demo` is the
 * broad feature fixture — 31 routes with plugins, every render strategy, and a
 * streamed document — and nothing had ever deployed it and asked it anything,
 * which is why the plugin lane could answer 500 for every 204 in a deployed
 * build with every check still green.
 */
const APPS = ['deploy-smoke', 'demo']
const appIndex = process.argv.indexOf('--app')
const app = appIndex === -1 ? 'deploy-smoke' : process.argv[appIndex + 1]
const positional = process.argv.slice(2).filter((value, index, all) => {
  const previous = all[index - 1]
  return value !== '--app' && previous !== '--app'
})
const [runtime, deploymentDirectory, portArg, assetsArg, platformConfigArg] = positional
if (!RUNTIMES.includes(runtime) || !deploymentDirectory || !portArg || !APPS.includes(app)) {
  console.error(
    `usage: node scripts/smoke-runtime-adapter.mjs <${RUNTIMES.join('|')}> <deployment-dir> <port>` +
      ' [publish-dir] [platform-config]',
  )
  console.error(
    '  edge takes the worker directory; assets default to <deployment-dir>/../assets, which is',
  )
  console.error('  where the cloudflare adapter puts the files its platform serves.')
  console.error(
    '  the four serverless lanes take the emitted function directory, the directory their',
  )
  console.error(
    '  platform publishes as static, and the config file that platform reads its asset headers',
  )
  console.error('  from — so the header assertion lands on the artifact, not on this script.')
  console.error(
    `  --app <${APPS.join('|')}> selects whose expectations to check; default deploy-smoke.`,
  )
  process.exit(2)
}

const port = Number(portArg)
const base = `http://127.0.0.1:${port}`

/**
 * The route strategies this deployment can serve — `adapter.supports`, verbatim.
 *
 * An edge target has no ISR, and a static publish directory has no server at
 * all, so the build refuses those routes with `RUV2202` and the fixture each is
 * built from has none of them. The checks that ask for one therefore have
 * nothing to ask. Skipped checks are printed rather than dropped: a suite that
 * quietly shrinks reads as one that passed.
 */
const CAPABILITIES_BY_RUNTIME = {
  edge: ['ssr', 'ssg', 'csr', 'api'],
  static: ['ssg', 'csr'],
}
const capabilities = CAPABILITIES_BY_RUNTIME[runtime]
  ? new Set(CAPABILITIES_BY_RUNTIME[runtime])
  : null

let output = ''
let child = null
let server = null

/**
 * Published files still being read, so exit can wait for them.
 *
 * A `ReadStream` closes its descriptor *after* the response it fed has ended,
 * on the libuv threadpool. `process.exit` while one of those completions is in
 * flight aborts the process on Windows — `Assertion failed:
 * !(handle->flags & UV_HANDLE_CLOSING), file src/win/async.c` — with every
 * check already reported green, which is exactly the shape that reads as a
 * flaky runner rather than as this script.
 */
const openFileStreams = new Set()

/** Send a published file, and remember it until its descriptor is closed. */
function sendFile(response, file) {
  const stream = createReadStream(file)
  openFileStreams.add(stream)
  const settle = () => openFileStreams.delete(stream)
  stream.once('close', settle)
  stream.once('error', settle)
  stream.pipe(response)
}

/** Wait for those descriptors, bounded — nothing here should take a second. */
async function settleFileStreams() {
  const deadline = Date.now() + 1_000
  while (openFileStreams.size > 0 && Date.now() < deadline) {
    await new Promise((resolve) => setImmediate(resolve))
  }
  for (const stream of openFileStreams) stream.destroy()
  openFileStreams.clear()
}

/**
 * Runtimes whose program is reached through a CDN rather than directly.
 *
 * Amplify's compute resource is only ever asked for what the `Static` route
 * targets did not answer, and its bundle carries no copy of the published files
 * — asking it for `/smoke.svg` is a 404 there and on Amplify alike. So this
 * lane spawns the program on an inner port and puts the platform's half in
 * front of it, which is also what makes the header assertion mean something:
 * the policy on a published file comes from `deploy-manifest.json` and
 * `customHttp.yml`, not from the server.
 */
const PROXIED = new Set(['aws'])
const innerPort = PROXIED.has(runtime) ? port + 1_000 : port

if (runtime === 'static') await startStaticSite()
else if (FETCH_MODULE.has(runtime)) await startFetchModule()
else if (NODE_HANDLER.has(runtime)) await startNodeHandler()
else {
  startRuntimeProcess()
  if (PROXIED.has(runtime)) await startStaticFront()
}

/** Spawn the emitted standalone server, which is a program on these runtimes. */
function startRuntimeProcess() {
  // Amplify's compute bundle is the same standalone server, emitted at the root
  // of the compute directory rather than under `server/` — its `server.js`
  // entrypoint is one line that imports `./index.mjs`. So the aws lane differs
  // from the node lane in the path and in nothing else, which is the claim.
  const entry = runtime === 'aws' ? 'index.mjs' : path.join('server', 'index.mjs')
  const executable = NODE_PROGRAM.has(runtime) ? 'node' : runtime
  const args = runtime === 'deno' ? ['run', '-A', '--no-prompt', entry] : [entry]
  child = spawn(runtimeExecutable(executable), args, {
    cwd: path.resolve(deploymentDirectory),
    env: { ...process.env, HOST: '127.0.0.1', PORT: String(innerPort) },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  child.stdout.on('data', (chunk) => (output += chunk))
  child.stderr.on('data', (chunk) => (output += chunk))
}

/** Serve the published files, then hand everything else to the spawned server. */
async function startStaticFront() {
  const assetsDir = publishDirectory()
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
        sendFile(response, file)
        return
      }
      await forwardToInnerServer(request, response, url)
    } catch (error) {
      output += `${error?.stack ?? error}\n`
      if (!response.headersSent) response.statusCode = 502
      response.end('compute unreachable')
    }
  })
  await new Promise((resolve) => server.listen(port, '127.0.0.1', resolve))
}

/** Pass a request through unchanged, including the headers the checks read. */
function forwardToInnerServer(request, response, url) {
  return new Promise((resolve, reject) => {
    const call = httpRequest(
      {
        host: '127.0.0.1',
        port: innerPort,
        path: url.pathname + url.search,
        method: request.method,
        headers: request.headers,
      },
      (inner) => {
        response.statusCode = inner.statusCode
        for (const [name, value] of Object.entries(inner.headers)) response.setHeader(name, value)
        inner.pipe(response)
        inner.on('end', resolve)
        inner.on('error', reject)
      },
    )
    call.on('error', reject)
    request.pipe(call)
  })
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
async function startFetchModule() {
  const workerDir = path.resolve(deploymentDirectory)
  const assetsDir = publishDirectory()
  const module = await import(pathToFileURL(path.join(workerDir, 'index.mjs')).href)
  // Cloudflare exports `{ fetch }`; a Netlify Functions v2 entry is the bare
  // async function, and its `config` export is read by the platform rather than
  // called. Both are the same contract once named: a `Request` in, a `Response`
  // out, and nothing else from the host.
  const worker =
    typeof module.default === 'function' ? { fetch: module.default } : (module.default ?? {})
  if (typeof worker?.fetch !== 'function') {
    throw new Error(`${workerDir}/index.mjs exports neither { fetch } nor a request function`)
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
        sendFile(response, file)
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

/**
 * Serve the publish directory and nothing else, the way a static host does.
 *
 * The `static` target emits no server of any kind, so there is no artifact here
 * to spawn or import — the deployment *is* the directory, and every claim it
 * makes is a file in it. Two of those claims have no other lane that can check
 * them: `_headers`, which is the only mechanism this target has for a cache or
 * a security policy, and `404.html`, which is the only way the project's own
 * not-found page can be reached when nothing is running to read a manifest.
 *
 * A miss falls through to `404.html` with a 404 status because that is what
 * Netlify, Cloudflare Pages, GitHub Pages, and an S3 website all do with it. A
 * host that ignores the convention would show its own page — which is exactly
 * the difference this lane exists to keep visible.
 */
async function startStaticSite() {
  const publishDir = publishDirectory()
  const notFound = path.join(publishDir, '404.html')
  server = createServer((request, response) => {
    const url = new URL(request.url, base)
    const file = staticFile(publishDir, url.pathname)
    if (file) {
      response.statusCode = 200
      response.setHeader('content-type', assetContentType(file))
      for (const [name, value] of assetHeaders(url.pathname)) response.setHeader(name, value)
      sendFile(response, file)
      return
    }
    response.statusCode = 404
    if (!existsSync(notFound)) {
      response.end('Not Found')
      return
    }
    response.setHeader('content-type', 'text/html; charset=utf-8')
    for (const [name, value] of assetHeaders(url.pathname)) response.setHeader(name, value)
    sendFile(response, notFound)
  })
  await new Promise((resolve) => server.listen(port, '127.0.0.1', resolve))
}

/**
 * The directory this platform publishes as static, served before the function.
 *
 * Every one of these platforms answers a published file from its own network
 * and never invokes the function for it — the Vercel `handle: filesystem` step,
 * Netlify's publish directory, Amplify's `Static` route target, Firebase
 * Hosting's `public`. Serving it here in the same order is what makes
 * `/smoke.svg` and a hashed browser bundle resolve the way they will in
 * production instead of falling through to a 404 from the handler.
 */
function publishDirectory() {
  if (assetsArg) return path.resolve(assetsArg)
  return path.resolve(path.join(deploymentDirectory, '..', 'assets'))
}

/**
 * Put a real HTTP server in front of an emitted **Node request handler**.
 *
 * Vercel's Node function and Firebase's `onRequest` are both `(req, res)` — the
 * shape a Node server already speaks — so this lane calls them with the real
 * objects rather than a translation of them. That is the point: everything
 * between the socket and `createHandler` is adapter-written glue that no unit
 * test runs, and it is where a streamed body, a repeated `set-cookie`, or a
 * null-body status goes wrong.
 *
 * Firebase's handler is Express-shaped inside `onRequest`, so the two
 * properties it reads that Node does not provide — `req.rawBody` and
 * `res.status()` — are added here, exactly as Cloud Functions adds them.
 */
async function startNodeHandler() {
  const functionDir = path.resolve(deploymentDirectory)
  const assetsDir = publishDirectory()
  if (runtime === 'firebase') installFirebaseFunctionsStub(functionDir)
  const module = await import(pathToFileURL(path.join(functionDir, 'index.mjs')).href)
  const handler =
    runtime === 'firebase'
      ? Object.values(module).find((value) => typeof value === 'function')
      : module.default
  if (typeof handler !== 'function') {
    throw new Error(`${functionDir}/index.mjs exports no request handler`)
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
        sendFile(response, file)
        return
      }
      if (runtime === 'firebase') {
        request.originalUrl = request.url
        const chunks = []
        for await (const chunk of request) chunks.push(chunk)
        request.rawBody = chunks.length > 0 ? Buffer.concat(chunks) : undefined
        response.status = (code) => {
          response.statusCode = code
          return response
        }
      }
      await handler(request, response, {})
    } catch (error) {
      output += `${error?.stack ?? error}\n`
      if (!response.headersSent) response.statusCode = 500
      response.end('function threw')
    }
  })
  await new Promise((resolve) => server.listen(port, '127.0.0.1', resolve))
}

/**
 * A stand-in for the one import the Firebase entry makes.
 *
 * `onRequest(options, handler)` registers the handler with the Functions
 * runtime and returns it; on the platform that runtime is what listens. Nothing
 * here can supply the platform, and nothing in the artifact depends on it —
 * what is under test is the `(req, res)` body of the handler, which the real
 * `onRequest` also just hands back. Written into the emitted function's own
 * `node_modules`, which is a gitignored build directory the platform installs
 * into anyway.
 */
function installFirebaseFunctionsStub(functionDir) {
  const root = path.join(functionDir, 'node_modules', 'firebase-functions')
  mkdirSync(path.join(root, 'v2'), { recursive: true })
  writeFileSync(
    path.join(root, 'package.json'),
    // An explicit `exports` map, because ESM has no extension search: without it
    // `firebase-functions/v2/https` resolves to a path with no file at it.
    JSON.stringify({
      name: 'firebase-functions',
      version: '0.0.0-smoke',
      type: 'module',
      exports: { './v2/https': './v2/https.js' },
    }),
  )
  writeFileSync(
    path.join(root, 'v2', 'https.js'),
    'export const onRequest = (_options, handler) => handler\n',
  )
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
  // `firstMatch` rules stop at the first one that matches; the rest merge, later
  // winning. Both semantics are real and the difference is not cosmetic: an
  // Amplify deploy manifest is a **routing table**, where the first matching
  // route handles the request, while a `_headers` file and the Vercel, Netlify,
  // and Firebase header blocks are lists that accumulate. Merging the manifest
  // like the others made `/__ruvyxa/client/<hash>.js` match `/*.*` after
  // `/__ruvyxa/client/*` and reported the immutable bundle as revalidating
  // hourly — a defect in this file, read as a defect in the adapter.
  let routed = false
  for (const rule of headerRules) {
    if (rule.firstMatch && routed) continue
    if (!rule.pattern.test(pathname)) continue
    if (rule.firstMatch) routed = true
    for (const [name, value] of rule.headers) matched.set(name, value)
  }
  return matched
}

let headerRules = null

function parseHeaderRules() {
  // `edge` and `static` are the two targets whose adapter writes a `_headers`
  // file; every other platform is configured through a JSON or YAML file of its
  // own shape.
  if (runtime !== 'edge' && runtime !== 'static') return parsePlatformHeaderRules()
  const file = path.join(publishDirectory(), '_headers')
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

/**
 * The cache policy a platform attaches to a published file, read from the file
 * that platform reads it from.
 *
 * Restating the expected header here would prove only that this script agrees
 * with itself. An adapter's whole static story is a config the platform
 * interprets — `.vercel/output/config.json`, `.netlify/v1/config.json`,
 * Amplify's `deploy-manifest.json`, `firebase.json` — so the rules are parsed
 * out of the emitted artifact and applied in that platform's own order.
 */
function parsePlatformHeaderRules() {
  const file = platformConfigArg ? path.resolve(platformConfigArg) : null
  if (!file || !existsSync(file)) return []
  const config = JSON.parse(readFileSync(file, 'utf8'))
  if (runtime === 'vercel') {
    // The `continue: true` rules only: the ones that attach headers and let
    // routing carry on to `handle: filesystem`.
    return (config.routes ?? [])
      .filter((route) => route.headers && route.continue)
      .map((route) => ({
        pattern: new RegExp(route.src),
        headers: Object.entries(route.headers).map(([name, value]) => [name.toLowerCase(), value]),
      }))
  }
  if (runtime === 'netlify') {
    return (config.headers ?? []).map((rule) => ({
      pattern: globToRegExp(rule.for),
      headers: Object.entries(rule.values).map(([name, value]) => [name.toLowerCase(), value]),
    }))
  }
  if (runtime === 'firebase') {
    return (config.hosting?.headers ?? []).map((rule) => ({
      pattern: globToRegExp(rule.source),
      headers: rule.headers.map((entry) => [entry.key.toLowerCase(), entry.value]),
    }))
  }
  // Amplify names the cache policy on the route itself rather than in a header
  // block, and only a `Static` target carries one. Everything else it attaches
  // comes from `customHttp.yml`, which is a separate file beside the app — so
  // both are read, in the order Amplify applies them.
  return [
    ...parseAmplifyCustomHttp(path.join(path.dirname(file), '..', 'customHttp.yml')),
    ...(config.routes ?? [])
      .filter((route) => route.target?.kind === 'Static' && route.target.cacheControl)
      .map((route) => ({
        pattern: globToRegExp(route.path),
        firstMatch: true,
        headers: [['cache-control', route.target.cacheControl]],
      })),
  ]
}

/**
 * The `customHeaders` blocks of an Amplify `customHttp.yml`.
 *
 * A purpose-built reader for the exact shape the adapter emits rather than a
 * YAML dependency: this asserts the file the adapter wrote, and a file it did
 * not write would simply match nothing here and fail the check that needs it.
 */
function parseAmplifyCustomHttp(file) {
  if (!existsSync(file)) return []
  const rules = []
  let key = null
  for (const line of readFileSync(file, 'utf8').split('\n')) {
    const pattern = line.match(/^\s*-\s*pattern:\s*'?([^'\n]+?)'?\s*$/)
    if (pattern) {
      rules.push({ pattern: globToRegExp(pattern[1]), headers: [] })
      continue
    }
    const named = line.match(/^\s*-\s*key:\s*'?([^'\n]+?)'?\s*$/)
    if (named) {
      key = named[1].toLowerCase()
      continue
    }
    const value = line.match(/^\s*value:\s*'?([^'\n]*?)'?\s*$/)
    if (value && key && rules.length > 0) {
      rules.at(-1).headers.push([key, value[1]])
      key = null
    }
  }
  return rules
}

/** `*` and `**` match any run of characters; everything else is literal. */
function globToRegExp(pattern) {
  const escaped = pattern.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*+/g, '.*')
  return new RegExp(`^${escaped}$`)
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
    requires: 'api',
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
    // The status *and* the body. A host has a 404 page of its own and will show
    // it perfectly happily, so a status-only assertion passes whether or not the
    // application's `not-found.tsx` ever reached the deployment. Each target
    // carries it a different way — a function reads `notFoundDocument` out of
    // the manifest, a static publish directory has only the `404.html`
    // convention — and neither is exercised by anything else here.
    name: 'an unknown path is the project’s own 404',
    path: '/definitely-not-a-route',
    assert: (response, body) => {
      if (response.status !== 404) return `status ${response.status}`
      if (!body.includes('SMOKE-NOT-FOUND-MARKER')) {
        return `the host's own 404 was served, not the project's: ${body.slice(0, 120)}`
      }
      return null
    },
  },
  {
    // The one asset rule no other check reads, and the one a mistake is silent
    // in: a glob that crosses path separators matches a hashed bundle too and
    // quietly replaces `immutable` with the one-hour public-asset lifetime.
    // Every visitor then re-fetches every bundle on every navigation. Asked of
    // every lane, because each sets it somewhere different — a platform config
    // file on the CDN targets, `serveStatic` in the standalone server.
    name: 'a hashed client bundle is immutable',
    path: '/',
    assert: async (response, body) => {
      const bundle = body.match(/src="(\/__ruvyxa\/client\/[^"]+)"/)
      if (!bundle) return 'the document names no browser bundle'
      const asset = await fetch(base + bundle[1])
      await asset.arrayBuffer()
      if (asset.status !== 200) return `${bundle[1]} answered ${asset.status}`
      const cache = asset.headers.get('cache-control')
      if (cache !== 'public, max-age=31536000, immutable') return `cache-control ${cache}`
      return null
    },
  },
  {
    // A dynamic server-components route is rendered by the emitted function on
    // every request, so this is the only check that exercises what a deployed
    // build has to carry for itself: the `react-server` graph, the SSR registry
    // that turns a reference id back into a component, and the payload data
    // block the browser hydrates from. Every adapter refused this shape with
    // RUV2213 until the route registry learned to render through that pipeline.
    name: 'GET /rsc renders a dynamic server-components route',
    requires: 'ssr',
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
 * What `examples/demo` adds, which is everything the small fixture has no way
 * to reach.
 *
 * `deploy-smoke` is four routes and no plugins, so the whole plugin lane, every
 * render strategy but two, and a streamed document were deployed by nobody and
 * asked by nothing. A response hook that turned every 204 into a 500 lived in
 * that gap; so did a deployed build that never streamed. These checks are
 * chosen for what a deployment carries *from the application* rather than from
 * the framework.
 */
const DEMO_CHECKS = [
  {
    // Both demo plugins are `http.onResponse` registrations that rebuild the
    // response to add a header — the pattern the documentation shows and the
    // one that used to make a null-body status a 500. Nothing had ever run a
    // plugin inside a deployed function.
    name: 'a plugin response hook runs in the deployed function',
    path: '/plugin-lab',
    assert: (response) => {
      if (response.status !== 200) return `status ${response.status}`
      if (response.headers.get('x-demo-plugin-response') !== 'active') {
        return `x-demo-plugin-response ${response.headers.get('x-demo-plugin-response')}`
      }
      if (response.headers.get('x-demo-plugin-route') !== '/plugin-lab') {
        return `x-demo-plugin-route ${response.headers.get('x-demo-plugin-route')}`
      }
      return null
    },
  },
  {
    // The other half of a `match` list, and the half a hook that ran on
    // everything would pass silently.
    name: 'a plugin hook stays off the routes it does not match',
    path: '/about',
    assert: (response) => {
      if (response.status !== 200) return `status ${response.status}`
      const header = response.headers.get('x-demo-plugin-response')
      if (header !== null) return `x-demo-plugin-response leaked as ${header}`
      return null
    },
  },
  ...[
    ['/static-page', 'static'],
    ['/ssg-blog', 'ssg'],
    ['/isr-page', 'isr'],
    ['/csr-page', 'csr'],
    ['/ppr-page', 'ppr'],
  ].map(([path, mode]) => ({
    // One page per render strategy, each answered by a different branch of the
    // handler, and each carrying a header only a plugin could have added — so
    // this asserts the strategy *and* that the plugin lane runs on all of them.
    name: `${path} renders as ${mode} with its plugin badge`,
    path,
    assert: (response) => {
      if (response.status !== 200) return `status ${response.status}`
      const badge = response.headers.get('x-demo-render-mode')
      if (badge !== mode) return `x-demo-render-mode ${badge}`
      return null
    },
  })),
  {
    name: 'a dynamic segment reaches the page as a parameter',
    path: '/showcase/hello',
    assert: (response, body) =>
      response.status === 200 && body.includes('hello') ? null : `status ${response.status}`,
  },
  {
    name: 'a catch-all route answers a path of any depth',
    path: '/catchall/a/b/c/d',
    assert: (response) => (response.status === 200 ? null : `status ${response.status}`),
  },
  {
    name: 'a build-time static parameter is served',
    path: '/ssg-blog/first-post',
    assert: (response) => (response.status === 200 ? null : `status ${response.status}`),
  },
  {
    name: 'an unknown path is the project’s own 404',
    path: '/definitely-not-a-route',
    assert: (response, body) => {
      if (response.status !== 404) return `status ${response.status}`
      if (!/not found/i.test(body)) return `body ${body.slice(0, 120)}`
      return null
    },
  },
  {
    name: 'a served page carries the security defaults',
    path: '/',
    assert: (response) => {
      if (response.headers.get('x-content-type-options') !== 'nosniff') return 'no nosniff'
      if (response.headers.get('x-frame-options') !== 'DENY') return 'no frame-options'
      return null
    },
  },
  {
    name: 'a hashed client bundle is immutable',
    path: '/',
    assert: async (response, body) => {
      const bundle = body.match(/src="(\/__ruvyxa\/client\/[^"]+)"/)
      if (!bundle) return 'the document names no browser bundle'
      const asset = await fetch(base + bundle[1])
      await asset.arrayBuffer()
      if (asset.status !== 200) return `${bundle[1]} answered ${asset.status}`
      const cache = asset.headers.get('cache-control')
      if (cache !== 'public, max-age=31536000, immutable') return `cache-control ${cache}`
      return null
    },
  },
]

/**
 * Something only the home page of *this* fixture puts in its markup.
 *
 * The compression check reads the decompressed bytes back, so it needs a string
 * it can recognise; the two fixtures share no markup, and asserting a
 * deploy-smoke marker against the demo made a passing gzip look like a broken
 * one.
 */
const HOME_MARKER = app === 'demo' ? '<h1>' : 'data-smoke="page"'

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
  if (!decoded.includes(HOME_MARKER)) {
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
  if (!identity.body.toString('utf8').includes(HOME_MARKER)) {
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

/**
 * Every asset every page names, fetched.
 *
 * A document that references a stylesheet or a bundle the deployment did not
 * carry still renders, still answers 200, and still looks right in a status
 * table — it is just unstyled and inert. Asking one page proves nothing about
 * the other thirty, because what each references depends on which components it
 * pulled in.
 */
async function checkEveryPageAsset(paths) {
  const seen = new Map()
  for (const pathname of paths) {
    const document = await rawGet(pathname, {})
    if (document.status !== 200) {
      throw new Error(`${runtime}: ${pathname} answered ${document.status}\n${output}`)
    }
    const html = document.body.toString('utf8')
    for (const match of html.matchAll(/(?:src|href)="(\/[^"]+)"/g)) {
      const asset = match[1]
      if (!asset.startsWith('/__ruvyxa/') && !/\.(?:js|mjs|css|svg|png|webp|woff2?)$/.test(asset)) {
        continue
      }
      if (!seen.has(asset)) seen.set(asset, (await rawGet(asset, {})).status)
    }
  }
  const broken = [...seen].filter(([, status]) => status !== 200)
  if (broken.length > 0) {
    throw new Error(
      `${runtime}: ${broken.length} referenced asset(s) do not resolve: ${broken
        .map(([asset, status]) => `${status} ${asset}`)
        .join(', ')}\n${output}`,
    )
  }
  console.log(
    `[ok] ${runtime} · every asset named by ${paths.length} pages resolves (${seen.size} unique)`,
  )
}

/**
 * The shell leaves before the slow half of the page has rendered.
 *
 * Asserted as *chunks over time* rather than as bytes, because the finished
 * document is byte-identical either way — which is exactly how a deployed build
 * that buffered every render passed every other check here while its first byte
 * arrived a second and a quarter late.
 */
function checkStreaming(pathname) {
  return new Promise((resolve, reject) => {
    const started = Date.now()
    const arrivals = []
    const call = httpRequest(
      { host: '127.0.0.1', port, path: pathname, headers: { 'accept-encoding': 'identity' } },
      (response) => {
        response.on('data', () => arrivals.push(Date.now() - started))
        response.on('end', () => {
          if (arrivals.length < 2) {
            reject(
              new Error(
                `${runtime}: ${pathname} arrived in one chunk after ${arrivals[0]}ms — the document was buffered\n${output}`,
              ),
            )
            return
          }
          const first = arrivals[0]
          const last = arrivals.at(-1)
          if (last - first < 100) {
            reject(
              new Error(
                `${runtime}: ${pathname} arrived all at once (${first}ms → ${last}ms); the slow boundary should be ~900ms behind the shell`,
              ),
            )
            return
          }
          console.log(
            `[ok] ${runtime} · ${pathname} streams — shell at ${first}ms, last chunk at ${last}ms`,
          )
          resolve()
        })
        response.on('error', reject)
      },
    )
    call.on('error', reject)
    call.end()
  })
}

/** A server action, over HTTP, in the shape the browser sends it. */
async function checkServerAction() {
  const called = await rawPost(
    '/__ruvyxa/action?path=/todos&name=createTodo',
    { 'content-type': 'application/json', origin: base },
    JSON.stringify({ input: { title: 'from-smoke' } }),
  )
  if (called.status !== 200) {
    throw new Error(
      `${runtime}: server action answered ${called.status}: ${called.body.toString('utf8').slice(0, 200)}\n${output}`,
    )
  }
  const answer = called.body.toString('utf8')
  if (!answer.includes('from-smoke')) {
    throw new Error(`${runtime}: server action returned ${answer.slice(0, 200)}`)
  }
  if (!answer.includes('todos')) {
    throw new Error(`${runtime}: server action reported no invalidation: ${answer.slice(0, 200)}`)
  }

  // The header a cross-origin page cannot set without a preflight. Without this
  // the endpoint is a write primitive for any site the visitor also has open.
  const unguarded = await rawPost(
    '/__ruvyxa/action?path=/todos&name=createTodo',
    { 'content-type': 'application/json' },
    JSON.stringify({ input: { title: 'from-nowhere' } }),
  )
  if (unguarded.status !== 403) {
    throw new Error(`${runtime}: an action with no Origin answered ${unguarded.status}`)
  }
  console.log(`[ok] ${runtime} · a server action runs and refuses a cross-origin caller`)
}

try {
  await waitUntilServing()
  let ran = 0
  for (const check of app === 'demo' ? DEMO_CHECKS : checks) {
    if (check.requires && capabilities && !capabilities.has(check.requires)) {
      console.log(`[skip] ${runtime} · ${check.name} — this target has no ${check.requires}`)
      continue
    }
    const response = await fetch(base + check.path)
    const body = await response.text()
    const failure = await check.assert(response, body)
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
  // convenience. The same is true of every serverless function: the platform's
  // CDN negotiates in front of it, and the function returns identity bytes on
  // purpose. `aws` joins them for a different reason — its compute bundle *is*
  // the standalone server and does negotiate, but the document this check asks
  // for is a `Static` route Amplify answers from its own edge, so what the
  // check would measure is the CDN stand-in rather than the artifact. The
  // negotiating code is the same `standaloneServerSource` the node, bun, and
  // deno lanes hold to the claim directly.
  if (!SPAWNED.has(runtime) || PROXIED.has(runtime)) {
    console.log(`[skip] ${runtime} · content negotiation — the platform owns it on this target`)
  } else {
    await checkCompression()
    ran += 2
  }
  // Both are the server-components endpoint, which a target with no server
  // does not have. `adapter.supports` says so, and the fixture that target is
  // built from has no such route to ask about.
  if (app === 'demo') {
    // The broad fixture has no /rsc route of its own, and its value is in what
    // the application carries rather than in the framework endpoints the small
    // fixture already covers on every adapter.
    await checkEveryPageAsset([
      '/',
      '/plugin-lab',
      '/server-components',
      '/streaming',
      '/gallery',
      '/todos',
      '/csr-page',
      '/ppr-page',
    ])
    await checkStreaming('/streaming')
    await checkServerAction()
    ran += 3
  } else if (capabilities && !capabilities.has('ssr')) {
    console.log(`[skip] ${runtime} · /__ruvyxa/rsc — this target has no ssr`)
  } else {
    await checkRscPayload()
    await checkServerFunction()
    ran += 2
  }
  console.log(`[ok] ${runtime} deployment artifact passed ${ran} checks`)
} finally {
  await stopServing()
}

// Let the loop drain, and force an exit only if it will not.
//
// An imported artifact is a module its platform never asks to let a process
// exit — Cloudflare has no process to exit, and a serverless function is
// invoked rather than run — so this used to call `process.exit(0)`
// unconditionally. That is what made the Windows runner abort with
// `Assertion failed: !(handle->flags & UV_HANDLE_CLOSING), file
// src/win/async.c` *after* printing every check green: exiting while a libuv
// threadpool completion is in flight aborts rather than exits, and a suite that
// passed then died at 127 reads as a flaky runner.
//
// So the handles this script owns are closed first and the process is allowed
// to end on its own. The timer is the guard for the case that argument was
// written for, and it is `unref`d — it cannot keep the loop alive itself, and
// only fires if something else already has. A thrown check still exits non-zero
// above this line.
if (!SPAWNED.has(runtime)) {
  keepAliveAgents().forEach((agent) => agent.destroy())
  setTimeout(() => process.exit(0), 2_000).unref()
}

/**
 * Every pooled HTTP client this script opened, including the one it did not.
 *
 * `fetch` keeps its sockets in a global dispatcher with a keep-alive timeout,
 * which holds the loop open for seconds after the last check. Node exposes no
 * public handle on it; the well-known symbol is how it is reached, and the
 * optional chaining is what keeps this correct on a runtime that has no undici.
 */
function keepAliveAgents() {
  const dispatcher = globalThis[Symbol.for('undici.globalDispatcher.1')]
  return [dispatcher].filter((agent) => typeof agent?.destroy === 'function')
}

/** Stop whichever transport was started, without leaving the port held. */
async function stopServing() {
  await settleFileStreams()
  if (server) {
    // `closeAllConnections` before `close`, because `fetch` keeps its sockets
    // alive and `close` waits for every one of them. Without it the script
    // finished its checks and then hung with nothing left to do.
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
    // A proxied lane has both: the front server and the program behind it.
    if (!child) return
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
      // `/` rather than the API route: a static publish directory has no API,
      // and every target has a pre-rendered index.
      //
      // A proxied lane is polled on its **inner** port, past the static front.
      // The front is listening before the program behind it is, and `/` is a
      // published file it answers 200 for on its own — so polling through it
      // reported "serving" while the compute resource was still booting, and
      // every check that needed it failed with a 502.
      const response = await fetch(`http://127.0.0.1:${innerPort}/`)
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
