/**
 * Drive a real `ruvyxa dev` process, edit the project under it, and check what
 * the browser would have been told.
 *
 * Every other end-to-end lane in this repository exercises a *build*:
 * `scripts/smoke-runtime-adapter.mjs` launches the eleven deployment artifacts,
 * and the Rust suites cover the dev server's pieces — `hmr_tracker`, `watcher`,
 * the router, the render cache — one unit at a time. Nothing started the
 * command developers actually run all day and watched an edit travel from the
 * filesystem to the socket, so the seam between those pieces was covered by
 * nobody: a watcher that stops emitting, a tracker that classifies an edit
 * wrongly, a sequence number that stops increasing, and a socket that never
 * sends anything at all all leave every unit test green.
 *
 * Usage:
 *
 *     node scripts/smoke-dev-server.mjs <appRoot> <port>
 *
 * `RUVYXA_CLI` names a prebuilt binary; without it the workspace CLI is run
 * through `cargo run`, which is what the deployment lanes already do.
 *
 * Every edit is reverted in a `finally`, and the run fails if a source file is
 * not byte-identical afterwards — a smoke test that leaves the tree dirty is a
 * smoke test nobody will run twice.
 */
import { spawn } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'

const [appRootArg, portArg] = process.argv.slice(2)
if (!appRootArg || !portArg) {
  console.error('usage: node scripts/smoke-dev-server.mjs <appRoot> <port>')
  process.exit(1)
}

const appRoot = path.resolve(appRootArg)
const port = Number(portArg)
const origin = `http://127.0.0.1:${port}`

/** The HMR wire contract both ends are held to. */
const contract = JSON.parse(
  readFileSync(new URL('../tests/fixtures/hmr-contract.json', import.meta.url), 'utf8'),
)

/** Source files this run edits, with the bytes they must be restored to. */
const originals = new Map()

let output = ''
let child = null

function ok(message) {
  console.log(`[ok] dev · ${message}`)
}

function fail(message) {
  throw new Error(`dev: ${message}\n--- server output ---\n${output}`)
}

function start() {
  const prebuilt = process.env.RUVYXA_CLI
  const [command, args] = prebuilt
    ? [prebuilt, ['dev', '--root', appRoot, '--port', String(port)]]
    : [
        'cargo',
        ['run', '-q', '-p', 'ruvyxa_cli', '--', 'dev', '--root', appRoot, '--port', String(port)],
      ]
  child = spawn(command, args, {
    cwd: path.resolve(new URL('..', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')),
    env: { ...process.env, NO_COLOR: '1', RUVYXA_TELEMETRY: '0' },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  child.stdout.on('data', (chunk) => {
    output += chunk
  })
  child.stderr.on('data', (chunk) => {
    output += chunk
  })
}

async function waitUntilServing() {
  // A dev server compiles the project before it answers, and a cold `cargo run`
  // has to build the CLI first, so this window is wider than a deployment
  // lane's. It still fails fast when the process dies.
  const deadline = Date.now() + 240_000
  let lastError
  while (Date.now() < deadline) {
    if (child?.exitCode !== null && child?.exitCode !== undefined) {
      fail(`server exited with ${child.exitCode}`)
    }
    try {
      const response = await fetch(`${origin}/`)
      await response.arrayBuffer()
      return
    } catch (error) {
      lastError = error
      await new Promise((resolve) => setTimeout(resolve, 250))
    }
  }
  fail(`server never answered: ${lastError}`)
}

/** Read a project file once, remembering the bytes to put back. */
function source(relative) {
  const file = path.join(appRoot, relative)
  if (!originals.has(file)) originals.set(file, readFileSync(file, 'utf8'))
  return { file, text: originals.get(file) }
}

function edit(relative, transform) {
  // `source` remembers the pristine bytes for the restore; the transform runs
  // against what is on disk *now*, so a second edit builds on the first rather
  // than rewriting the original and silently changing nothing.
  const { file } = source(relative)
  const current = readFileSync(file, 'utf8')
  const next = transform(current)
  if (next === current) {
    fail(`the edit to ${relative} changed nothing, so no watcher event can follow`)
  }
  writeFileSync(file, next, 'utf8')
}

function restoreAll() {
  for (const [file, text] of originals) writeFileSync(file, text, 'utf8')
}

/**
 * Collect HMR messages until `predicate` accepts one.
 *
 * The socket is opened once and kept, because the sequence numbers are only
 * meaningful across one connection — a reconnect restarts them, and the stale
 * policy this contract names is written in terms of the last applied sequence.
 */
function hmrSocket() {
  const socket = new WebSocket(`ws://127.0.0.1:${port}/__ruvyxa/hmr`)
  const received = []
  const waiters = []
  socket.addEventListener('message', (event) => {
    let message
    try {
      message = JSON.parse(String(event.data))
    } catch {
      return
    }
    received.push(message)
    // Chosen before any of them is removed: settling a waiter mutates the list
    // being walked, and one message can satisfy more than one.
    const matched = waiters.filter((waiter) => waiter.predicate(message))
    for (const waiter of matched) {
      waiters.splice(waiters.indexOf(waiter), 1)
      waiter.resolve(message)
    }
  })

  const open = new Promise((resolve, reject) => {
    socket.addEventListener('open', () => resolve())
    socket.addEventListener('error', () => reject(new Error('the HMR socket refused to open')))
  })

  return {
    open,
    received,
    close: () => socket.close(),
    next(predicate, label, timeoutMs = 30_000) {
      const already = received.find(predicate)
      if (already) return Promise.resolve(already)
      return new Promise((resolve, reject) => {
        const waiter = { predicate, resolve }
        waiters.push(waiter)
        setTimeout(() => {
          if (!waiters.includes(waiter)) return
          waiters.splice(waiters.indexOf(waiter), 1)
          reject(
            new Error(
              `no HMR message for ${label} within ${timeoutMs}ms; saw ${JSON.stringify(received)}`,
            ),
          )
        }, timeoutMs).unref()
      })
    },
  }
}

async function text(pathname, init) {
  const response = await fetch(`${origin}${pathname}`, init)
  return { response, body: await response.text() }
}

async function checkServesTheProject() {
  const { response, body } = await text('/')
  if (response.status !== 200) fail(`GET / answered ${response.status}`)
  if (!body.includes('<!DOCTYPE html>') && !body.includes('<!doctype html>')) {
    fail('GET / did not answer a document')
  }
  ok('the project renders')

  const missing = await text('/definitely-not-a-route')
  if (missing.response.status !== 404) fail(`an unknown path answered ${missing.response.status}`)
  // The project's own not-found page, not the framework's bare string — the
  // same thing every deployment lane asserts, on the host developers see first.
  if (!missing.body.includes('not-found')) fail('an unknown path did not render app/not-found.tsx')
  ok('an unknown path is the project’s own 404')

  const asset = await fetch(`${origin}/smoke.svg`)
  await asset.arrayBuffer()
  if (asset.status !== 200) fail(`a public asset answered ${asset.status}`)
  if (!String(asset.headers.get('content-type')).includes('image/svg+xml')) {
    fail(`a public asset answered content-type ${asset.headers.get('content-type')}`)
  }
  ok('a public asset is served with its own content type')

  const api = await text('/api/health')
  if (api.response.status !== 200) fail(`the API route answered ${api.response.status}`)
  JSON.parse(api.body)
  ok('an API route answers in dev')

  const manifest = await text('/__ruvyxa/client/route-manifest.json')
  if (manifest.response.status !== 200) fail('the client route manifest is missing')
  const routes = JSON.parse(manifest.body)
  if (!Array.isArray(routes.routes) || routes.routes.length === 0) {
    fail('the client route manifest names no routes')
  }
  ok('the client route manifest is served')
}

/** Every field the contract requires, on a message that claims the protocol. */
function checkShape(message, label) {
  if (message.protocol !== contract.protocol) {
    fail(`${label} announced protocol ${message.protocol}`)
  }
  if (message.protocolVersion !== contract.protocolVersion) {
    fail(`${label} announced protocol version ${message.protocolVersion}`)
  }
  for (const field of contract.requiredFields) {
    if (!(field in message)) fail(`${label} is missing the required field ${field}`)
  }
  const known = contract.messages.some(
    (entry) => entry.type === message.type && entry.kind === message.kind,
  )
  if (!known)
    fail(
      `${label} sent type/kind ${message.type}/${message.kind}, which the table has no entry for`,
    )
}

/**
 * Call one of the route's server functions the way the browser does.
 *
 * `/__ruvyxa/action` and `/__ruvyxa/rsc` are `native: always` in the endpoint
 * table, and the deployment lanes drive them on all eleven targets — but the
 * host developers see first drove them nowhere. That direction is exactly how
 * the endpoint table came to exist: `/__ruvyxa/action` was added to the Axum
 * router and never ported to `createHandler`, so every server action worked in
 * development and 404ed on every deployed build. Nothing was checking the
 * mirror image.
 */
async function checkServerFunctions() {
  const rsc = await text('/rsc')
  if (rsc.response.status !== 200) fail(`GET /rsc answered ${rsc.response.status}`)
  if (!rsc.body.includes('id="__ruvyxa-rsc"')) fail('the document carried no Flight payload')
  ok('a server-components route renders with its payload')

  // The reference is read out of the bundle the browser would load, rather than
  // assembled here: a hand-built id proves this script agrees with itself.
  let reference = null
  for (const url of await clientBundleUrls('/rsc')) {
    const response = await fetch(`${origin}${url}`)
    const source = await response.text()
    const moduleId = source.match(/ruv:s_[0-9a-f]{16}/)
    if (moduleId) {
      reference = `${moduleId[0]}#echo`
      break
    }
  }
  if (!reference) fail('no browser bundle for /rsc named a server function')

  const called = await fetch(`${origin}/__ruvyxa/rsc?path=/rsc`, {
    method: 'POST',
    headers: {
      'x-ruvyxa-rsc': '1',
      'x-ruvyxa-action': reference,
      // What `encodeReply` produces for a single string argument.
      'content-type': 'text/plain;charset=UTF-8',
    },
    body: '["smoke"]',
  })
  const answer = await called.text()
  if (called.status !== 200) fail(`the server function answered ${called.status}: ${answer}`)
  if (!answer.includes('server:smoke')) fail(`the server function returned ${answer.slice(0, 200)}`)
  ok('a server function runs and returns its value')

  const unguarded = await fetch(`${origin}/__ruvyxa/rsc?path=/rsc`, {
    method: 'POST',
    headers: { 'x-ruvyxa-rsc': '1', 'content-type': 'text/plain;charset=UTF-8' },
    body: '["smoke"]',
  })
  await unguarded.arrayBuffer()
  if (unguarded.status !== 400) {
    fail(`a call naming no reference answered ${unguarded.status} instead of 400`)
  }
  ok('a call naming no server function is refused')
}

async function checkHotUpdates() {
  const socket = hmrSocket()
  await socket.open
  ok('the HMR socket connects')

  // A server-rendered page: the tracker has to classify this as a route update
  // rather than a full restart, which is the difference between a preserved
  // client state and a reload.
  edit('app/page.tsx', (text) => text.replace('export default', '// hmr-smoke\nexport default'))
  const first = await socket.next(
    (message) => message.protocol === contract.protocol && message.type !== 'connected',
    'an edit to app/page.tsx',
  )
  checkShape(first, 'the first update')
  if (first.type === 'restart') {
    fail('editing a page body asked the browser to restart rather than patching the route')
  }
  ok(`a page edit is announced as ${first.type}/${first.kind}`)

  if (!Array.isArray(first.modules) || first.modules.length === 0) {
    fail('the update named no modules, so a client has nothing to apply')
  }
  const named = first.modules.some((entry) => String(entry).includes('page'))
  if (!named)
    fail(`the update named ${JSON.stringify(first.modules)}, none of them the edited file`)
  ok('the update names the module that changed')

  // The page has to keep rendering after the edit: an update the server cannot
  // serve is worse than no update at all.
  const after = await text('/')
  if (after.response.status !== 200) fail(`GET / answered ${after.response.status} after an edit`)
  ok('the project still renders after the edit')

  // A second edit must carry a higher sequence: the stale policy this contract
  // names is "reject anything at or below the last applied", so a sequence that
  // stops advancing makes every later update look stale and silently stops HMR.
  edit('app/page.tsx', (text) => text.replace('// hmr-smoke\n', '// hmr-smoke-again\n'))
  const second = await socket.next(
    (message) =>
      message.protocol === contract.protocol &&
      message.type !== 'connected' &&
      message.sequence > first.sequence,
    'a second edit to app/page.tsx',
  )
  checkShape(second, 'the second update')
  ok(`the sequence advances (${first.sequence} → ${second.sequence})`)

  // A client component is a different lane through the tracker: it reaches the
  // browser bundle rather than the server render.
  edit('app/rsc/counter.tsx', (text) => text.replace("'use client'", "'use client'\n// hmr-smoke"))
  const client = await socket.next(
    (message) =>
      message.protocol === contract.protocol &&
      message.type !== 'connected' &&
      message.sequence > second.sequence,
    'an edit to a client component',
  )
  checkShape(client, 'the client-component update')
  ok(`a client component edit is announced as ${client.type}/${client.kind}`)

  socket.close()
}

/** The client bundle URLs the document asks the browser to load. */
async function clientBundleUrls(pathname = '/') {
  const { body } = await text(pathname)
  const found = new Set()
  for (const match of body.matchAll(/["'](\/__ruvyxa\/client[^"']*)["']/g)) {
    found.add(match[1].replaceAll('&amp;', '&'))
  }
  return [...found]
}

async function checkRecoversFromABrokenEdit() {
  const socket = hmrSocket()
  await socket.open

  const before = await clientBundleUrls()
  if (before.length === 0) fail('the document referenced no client bundle to break')

  // A client bundle is built when the browser asks for it, not when the file is
  // saved — so the failure has no watcher event to travel on, and the check has
  // to make the request a browser would. That asymmetry is the whole reason
  // `hmr_issue_payload` exists: before it, the script answered 500, the browser
  // called that a load failure, and the document around it stayed a perfectly
  // ordinary 200 with nothing on the page working.
  edit('app/page.tsx', (text) => `${text}\nexport const broken = (`)
  await socket.next(
    (message) => message.protocol === contract.protocol && message.type !== 'connected',
    'the broken edit',
  )

  let refused = null
  for (const url of before) {
    const response = await fetch(`${origin}${url}`)
    await response.arrayBuffer()
    // The status is the whole signal. Sniffing the body for `console.error`
    // was the first attempt and it is wrong in both directions: the error stub
    // is built from that call, and so is any application module that logs.
    if (response.status >= 400) {
      refused = { url, status: response.status }
      break
    }
  }
  if (!refused) fail(`no client bundle refused to build for a file with a syntax error: ${before}`)
  ok(`a bundle that cannot build answers ${refused.status} rather than stale JavaScript`)

  const issues = await socket.next(
    (message) => message.protocol === contract.protocol && message.type === 'issues',
    'a failed bundle build',
  )
  checkShape(issues, 'the failure update')
  if (!Array.isArray(issues.issues) || issues.issues.length === 0) {
    fail('the failure update carried no issue for the overlay to show')
  }
  // Without this the page has a broken script and no way to say so: the
  // overlay is the only thing that turns a 500 on a script URL into something
  // the developer sees.
  ok(`the overlay is told (${issues.issues[0].code})`)

  // And the server has to come back from it without a restart.
  restoreAll()
  await socket.next(
    (message) =>
      message.protocol === contract.protocol &&
      message.type !== 'issues' &&
      message.sequence > issues.sequence,
    'the fix',
  )
  const recovered = await fetch(`${origin}${refused.url}`)
  await recovered.arrayBuffer()
  if (recovered.status !== 200) {
    fail(`the bundle still answered ${recovered.status} after the file was fixed`)
  }
  ok('the bundle builds again once the file is valid')

  socket.close()
}

function checkTreeIsClean() {
  for (const [file, expected] of originals) {
    if (readFileSync(file, 'utf8') !== expected) {
      fail(`${path.relative(appRoot, file)} was left edited`)
    }
  }
  ok('every edited file was restored')
}

let checks = 0
try {
  start()
  await waitUntilServing()
  await checkServesTheProject()
  checks += 5
  await checkServerFunctions()
  checks += 3
  await checkHotUpdates()
  checks += 6
  await checkRecoversFromABrokenEdit()
  checks += 3
  restoreAll()
  checkTreeIsClean()
  checks += 1
  console.log(`[ok] dev server passed ${checks} checks`)
} finally {
  restoreAll()
  child?.kill()
}
