import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import {
  createReadStream,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { connect as netConnect, createServer } from 'node:net'
import { homedir, tmpdir } from 'node:os'
import path from 'node:path'
import { Readable } from 'node:stream'
import { after, describe, it } from 'node:test'
import { pathToFileURL } from 'node:url'

import { repoPath } from '../../repo-root.js'
import { standaloneServerSource } from '../../../packages/@ruvyxa/core/dist/standalone-server.js'

const { HANDLER_RUNTIME_FILES } = (await import(
  pathToFileURL(repoPath('packages/ruvyxa/runtime/serverless-handler.mjs')).href
)) as { HANDLER_RUNTIME_FILES: readonly string[] }

/**
 * The bytes `public/clip.mp4` is written with. Ranges are asserted against the
 * exact slice, not against a length: an off-by-one in a `content-range` end is
 * inclusive-versus-exclusive, and a length check passes on both.
 */
const CLIP_BYTES = Buffer.from('0123456789')

type MutableGlobal = typeof globalThis & Record<string, unknown>

/** One request against a running standalone server, however it is being run. */
type Client = (pathname: string, init?: RequestInit) => Promise<Response>

/** The same, from a peer of the caller's choosing. Fetch transports only. */
type PeerClient = (pathname: string, peer: string, init?: RequestInit) => Promise<Response>

/**
 * The connection object Bun and Deno hand their fetch handler.
 *
 * One object satisfies both shapes: Bun asks its `Server` for the peer by
 * request, Deno carries it on the second argument. The emitted program reads
 * whichever its own runtime supplies, and this is what it reads it from.
 */
function connectionFrom(peer: string) {
  return {
    requestIP: () => ({
      address: peer,
      family: peer.includes(':') ? 'IPv6' : 'IPv4',
      port: 51_000,
    }),
    remoteAddr: { transport: 'tcp', hostname: peer, port: 51_000 },
  }
}

/**
 * Stage the directory an adapter's `function` artifact produces.
 *
 * The real `serverless-handler.mjs` and the modules it imports are copied in,
 * exactly as `materializeFunction` copies them, so what is under test is the
 * emitted server against the handler it will actually be deployed with — not
 * against a stand-in that agrees with it today. Only the route modules are
 * stubbed, because compiling a project is not what this is measuring.
 */
function stageDeployment(
  runtime: 'node' | 'bun' | 'deno',
  securityHeaders = true,
  {
    apiLimit,
    hangingRoute = false,
    trustedProxyIps,
  }: { apiLimit?: number; hangingRoute?: boolean; trustedProxyIps?: string[] } = {},
): string {
  const root = mkdtempSync(path.join(tmpdir(), `ruvyxa-standalone-${runtime}-`))
  const server = path.join(root, 'server')
  const publicDir = path.join(root, 'public')
  mkdirSync(server, { recursive: true })
  mkdirSync(path.join(publicDir, '__ruvyxa', 'client'), { recursive: true })

  for (const file of HANDLER_RUNTIME_FILES) {
    cpSync(repoPath('packages/ruvyxa/runtime', file), path.join(server, file))
  }

  const security: Record<string, unknown> = {}
  if (!securityHeaders) security.headers = false
  if (trustedProxyIps) security.trustedProxyIps = trustedProxyIps
  if (apiLimit !== undefined) security.apiLimit = apiLimit
  writeFileSync(
    path.join(server, 'index.mjs'),
    standaloneServerSource({
      runtime,
      runtimePolicy: Object.keys(security).length > 0 ? { security } : {},
    }),
    'utf8',
  )
  writeFileSync(
    path.join(server, 'manifest.mjs'),
    'export default ' +
      JSON.stringify({
        routes: [
          { id: 'home', path: '/', kind: 'page', file: 'home.tsx', render: { strategy: 'ssr' } },
          // What the handler was actually handed. A forwarded identity the
          // transport declined to believe has to be gone before this, and no
          // status code or body can show that — only the header list can.
          { id: 'whoami', path: '/whoami', kind: 'api', file: 'whoami.ts' },
          // A stream that stays open, which is what an application's own
          // server-sent-event route is. Nothing in this repository serves one,
          // so this is the only place the decision not to encode it is
          // observable.
          { id: 'events', path: '/events', kind: 'api', file: 'events.ts' },
          // A compressible payload whose route asked not to be re-encoded.
          { id: 'plain', path: '/plain', kind: 'api', file: 'plain.ts' },
        ],
      }) +
      '\n',
    'utf8',
  )
  writeFileSync(
    path.join(server, 'route-modules.mjs'),
    [
      'export async function loadRouteModule(routeId) {',
      '  if (routeId === "whoami") {',
      '    return {',
      '      GET: ({ request }) =>',
      '        new Response(JSON.stringify([...request.headers]), {',
      "          headers: { 'content-type': 'application/json' },",
      '        }),',
      '    }',
      '  }',
      '  if (routeId === "events") {',
      '    return {',
      '      GET: () =>',
      '        new Response(',
      '          new ReadableStream({',
      '            start(controller) {',
      // One small write and then nothing: far under the ~16 KB an encoder
      // holds before it flushes, which is the whole failure.
      '              controller.enqueue(new TextEncoder().encode("data: first\\n\\n"))',
      '            },',
      '          }),',
      "          { headers: { 'content-type': 'text/event-stream' } },",
      '        ),',
      '    }',
      '  }',
      '  if (routeId === "plain") {',
      '    return {',
      '      GET: () =>',
      // Well past COMPRESSION_MIN_BYTES and plainly compressible, so only the
      // `no-transform` directive can be what keeps it uncompressed.
      '        new Response("x".repeat(4096), {',
      '          headers: {',
      "            'content-type': 'text/plain; charset=utf-8',",
      "            'cache-control': 'public, max-age=60, no-transform',",
      '          },',
      '        }),',
      '    }',
      '  }',
      // A render that never settles: an await on something that never
      // resolves is what a hung upstream call looks like from here.
      hangingRoute
        ? '  return { render: () => new Promise(() => {}) }'
        : '  return { render: async ({ path: pathname }) =>\n' +
          '    `<!doctype html><title>${routeId}</title><p>${pathname}</p>` }',
      '}',
      'export async function loadActionModule() { return {} }',
      '// Matches what the generator emits for a project with no HTTP plugins.',
      'export const applyPluginHttp = undefined',
      '',
    ].join('\n'),
    'utf8',
  )

  writeFileSync(path.join(publicDir, '__ruvyxa', 'client', 'app.abc123.js'), 'export default 1\n')
  writeFileSync(path.join(publicDir, 'logo.png'), 'not-really-a-png')
  // Published as WebP only, which is what `image.keepOriginal: false` leaves
  // behind while the markup still asks for the original extension.
  writeFileSync(path.join(publicDir, 'photo.webp'), 'not-really-a-webp')
  // The other direction: `image.optimize: false` publishes the source untouched
  // with no `.webp` beside it, while `<Image>` rewrites every src to its
  // `.webp` URL unconditionally.
  writeFileSync(path.join(publicDir, 'source.png'), 'not-really-a-png')
  // Two sources one `.webp` URL could name. The build-time collision guard
  // refuses this and so does `resolve_public_asset`; a first-hit loop would
  // answer it by array order.
  writeFileSync(path.join(publicDir, 'ambiguous.png'), 'not-really-a-png')
  writeFileSync(path.join(publicDir, 'ambiguous.jpg'), 'not-really-a-jpeg')
  writeFileSync(path.join(publicDir, 'clip.mp4'), CLIP_BYTES)
  writeFileSync(path.join(publicDir, 'fallback.html'), '<!doctype html><title>fallback</title>')
  writeFileSync(path.join(root, 'secret.txt'), 'must never be served')

  return server
}

/** A free TCP port, released before the server under test binds it. */
function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = createServer()
    probe.on('error', reject)
    probe.listen(0, '127.0.0.1', () => {
      const address = probe.address()
      const port = typeof address === 'object' && address ? address.port : 0
      probe.close(() => resolve(port))
    })
  })
}

/**
 * The installed Bun or Deno binary, or `null`.
 *
 * Looked for on `PATH` and in the location each installer uses, because on
 * Windows both are commonly reachable only as a `.cmd` shim that
 * `spawn` cannot launch — which is the same reason `JavaScriptRuntime::executable`
 * resolves the real executable behind it. A runtime that is present is used;
 * one that is not falls back to the stand-ins below, and the suite title says
 * which happened.
 */
function installedRuntime(runtime: 'bun' | 'deno'): string | null {
  const home = homedir()
  const candidates =
    process.platform === 'win32'
      ? [`${runtime}.exe`, path.join(home, `.${runtime}`, 'bin', `${runtime}.exe`)]
      : [runtime, path.join(home, `.${runtime}`, 'bin', runtime)]
  for (const candidate of candidates) {
    const probe = spawnSync(candidate, ['--version'], { stdio: 'ignore' })
    if (probe.status === 0) return candidate
  }
  return null
}

/**
 * Run one emitted server the way a host runs it: as its own process, answering
 * real sockets.
 *
 * This is the only shape that proves the transport was wired up at all, so the
 * Node one always takes it and Bun and Deno take it whenever they are installed.
 */
async function startProcessServer(
  executable: string,
  args: string[],
  server: string,
  extraEnv: Record<string, string> = {},
  { drainable = false }: { drainable?: boolean } = {},
): Promise<{
  client: Client
  stop: () => void
  drain: () => void
  output: () => string
  port: number
}> {
  const port = await freePort()

  // Windows maps child.kill('SIGTERM') onto TerminateProcess, which gives the
  // program no chance to answer anything — so a drain test that only knows how
  // to send a real signal is skipped there, and CI on another platform becomes
  // the only place it ever runs. It stopped being run and started being
  // discovered: the drain shipped broken and a macOS runner found it.
  //
  // Raising the signal inside the child instead runs the very listener
  // onShutdownSignal registered, which is the half this repository owns. Signal
  // delivery is the operating system's half and is not what is under test.
  const drainOnStdin = drainable && process.platform === 'win32'
  const preload: string[] = []
  if (drainOnStdin) {
    const trigger = path.join(server, 'drain-trigger.mjs')
    writeFileSync(
      trigger,
      [
        "process.stdin.setEncoding('utf8')",
        "process.stdin.on('data', () => { process.emit('SIGTERM') })",
        '',
      ].join('\n'),
      'utf8',
    )
    preload.push('--import', pathToFileURL(trigger).href)
  }

  const child = spawn(executable, [...preload, ...args, path.join(server, 'index.mjs')], {
    cwd: server,
    env: { ...process.env, PORT: String(port), HOST: '127.0.0.1', ...extraEnv },
    // Always a pipe, so this stays the literal TypeScript narrows the stream
    // handles from. A child that never reads it is unaffected, and an unread
    // stdin pipe does not hold the event loop open.
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  let output = ''
  child.stdout.setEncoding('utf8')
  child.stderr.setEncoding('utf8')
  child.stdout.on('data', (chunk: string) => {
    output += chunk
  })
  child.stderr.on('data', (chunk: string) => {
    output += chunk
  })

  const deadline = Date.now() + 20_000
  while (!output.includes('listening on')) {
    if (child.exitCode !== null) {
      throw new Error(`the standalone server exited before listening:\n${output}`)
    }
    if (Date.now() > deadline) throw new Error(`the standalone server never listened:\n${output}`)
    await new Promise((resolve) => setTimeout(resolve, 50))
  }

  return {
    client: (pathname, init) => fetch(`http://127.0.0.1:${port}${pathname}`, init),
    stop: () => child.kill(),
    // The signal an orchestrator sends, as opposed to `stop()`, which is the
    // test runner giving up on the process.
    drain: () => {
      if (drainOnStdin) child.stdin?.write('drain\n')
      else child.kill('SIGTERM')
    },
    // Everything the process has written so far, which for a server is the
    // whole of its structured output.
    output: () => output,
    port,
  }
}

/**
 * Bun's and Deno's server and file APIs, standing in under Node.
 *
 * Installed for the lifetime of this file rather than around each import: the
 * emitted program reaches for `Bun.file` / `Deno.open` when a request arrives,
 * not when it loads, so a stand-in that is uninstalled after the import is gone
 * by the time it is needed. Neither global exists on Node, so nothing is being
 * shadowed.
 *
 * Deliberately the smallest thing that satisfies the two APIs the emitted
 * program uses. A program that reaches for a third one fails here rather than
 * on a user's host — which is the point, since neither runtime is assumed to be
 * installed and this does not claim to test either of them. What it tests is
 * the program Ruvyxa emits for them, which is the part this repository owns.
 */
let registeredHandler: ((request: Request) => Promise<Response>) | undefined
/**
 * The runtime's own last-resort handler, taken the same way.
 *
 * Bun spells it `error` and Deno spells it `onError`, and neither is reachable
 * through a socket from here: it fires only when the fetch handler throws,
 * which the emitted program has no external way to be made to do. Captured so
 * the response it produces can be asserted at all.
 */
let registeredErrorHook: ((error: unknown) => Response | Promise<Response>) | undefined
const globals = globalThis as MutableGlobal
globals.Bun = {
  // Synchronous like the real one, and a Blob like the real one: a `Bun.file`
  // is accepted by `new Response` and answers `.slice`.
  file: (file: string) => new Blob([readFileSync(file)]),
  serve: (options: {
    fetch: (request: Request) => Promise<Response>
    error?: (error: unknown) => Response | Promise<Response>
  }) => {
    registeredHandler = options.fetch
    registeredErrorHook = options.error
    return { stop: () => {} }
  },
}
globals.Deno = {
  open: async (file: string) => ({ readable: Readable.toWeb(createReadStream(file)) }),
  serve: (
    options: {
      onListen?: (address: { hostname: string; port: number }) => void
      onError?: (error: unknown) => Response | Promise<Response>
    },
    fetchHandler: (request: Request) => Promise<Response>,
  ) => {
    options.onListen?.({ hostname: '127.0.0.1', port: 0 })
    registeredHandler = fetchHandler
    registeredErrorHook = options.onError
    return { shutdown: async () => {} }
  },
}

/**
 * Load one emitted fetch-transport program and take the handler it registers.
 *
 * Serialized, because the handler arrives through a module-level variable and
 * two suites loading at once would each take the other's.
 */
let loading: Promise<unknown> = Promise.resolve()
function startFetchServer(
  server: string,
  extraEnv: Record<string, string> = {},
): Promise<{
  client: Client
  peerClient: PeerClient
  stop: () => void
  errorHook: ((error: unknown) => Response | Promise<Response>) | undefined
}> {
  const next = loading.then(async () => {
    // The emitted program reads its deadlines from the environment at load, and
    // this stand-in loads it in-process — so the variables have to be here
    // before the import and gone after it, or the next server to load inherits
    // them.
    const restore = Object.entries(extraEnv).map(
      ([name, value]) => [name, process.env[name], value] as const,
    )
    for (const [name, , value] of restore) process.env[name] = value
    // Signals and error events do not share an overload on `process`, and this
    // needs to treat them alike.
    const emitter = process as unknown as {
      listeners(event: string): (() => void)[]
      off(event: string, listener: () => void): void
    }
    const events = ['uncaughtException', 'unhandledRejection', 'SIGTERM', 'SIGINT']
    const owned = new Map(events.map((event) => [event, new Set(emitter.listeners(event))]))
    registeredHandler = undefined
    registeredErrorHook = undefined
    try {
      await import(pathToFileURL(path.join(server, 'index.mjs')).href)
    } finally {
      for (const [name, previous] of restore) {
        if (previous === undefined) delete process.env[name]
        else process.env[name] = previous
      }
      // The emitted program installs process-wide handlers, and its
      // `uncaughtException` handler exits the process. Loading it into a test
      // runner must not leave that behind for whatever runs next.
      for (const event of events) {
        for (const listener of emitter.listeners(event)) {
          if (!owned.get(event)?.has(listener)) emitter.off(event, listener)
        }
      }
    }
    // Read through an explicit type: the assignment above is the last one the
    // compiler can see, so it would otherwise narrow this to `undefined` and
    // the assertion below to `never`.
    const handler = registeredHandler as
      ((request: Request, connection?: unknown) => Promise<Response>) | undefined
    assert.ok(handler, `${server} never registered a fetch handler`)
    return {
      // The handler these runtimes register takes a `Request`, which is the
      // whole point of the transport: there is nothing between the socket and
      // it.
      client: (pathname: string, init?: RequestInit) =>
        handler(new Request(`http://127.0.0.1${pathname}`, init)),
      // The second argument is the only place the peer exists on these two
      // runtimes, which is exactly why the file used to make no trust decision
      // at all. A stubbed peer is the only way to reach a non-loopback one: a
      // real socket in a test comes from 127.0.0.1, and loopback is trusted.
      peerClient: (pathname: string, peer: string, init?: RequestInit) =>
        handler(new Request(`http://127.0.0.1${pathname}`, init), connectionFrom(peer)),
      stop: () => {},
      errorHook: registeredErrorHook,
    }
  })
  loading = next.catch(() => {})
  return next
}

const runtimes = ['node', 'bun', 'deno'] as const

/** How each runtime's emitted program is launched, when it can be launched. */
const executables = {
  node: { executable: process.execPath, args: [] as string[] },
  bun: (() => {
    const executable = installedRuntime('bun')
    return executable ? { executable, args: [] as string[] } : null
  })(),
  deno: (() => {
    const executable = installedRuntime('deno')
    // The same flags the adapter's README tells an operator to use: the server
    // reads files, reads the environment, and listens.
    return executable ? { executable, args: ['run', '-A', '--no-prompt'] } : null
  })(),
} as const

for (const runtime of runtimes) {
  const launcher = executables[runtime]
  // Named so the report says which was measured. A stubbed run still checks the
  // program Ruvyxa emits — which is the part this repository owns — but it
  // cannot see what the runtime itself does with it, and that is exactly where
  // `BunFile.slice(…).stream()` turned out to serve a whole file.
  describe(`generated standalone server (${runtime}, ${launcher ? 'installed' : 'stubbed'})`, () => {
    const server = stageDeployment(runtime)
    let started: { client: Client; stop: () => void } | undefined
    let request: Client

    const ready = (async () => {
      started = launcher
        ? await startProcessServer(launcher.executable, launcher.args, server)
        : await startFetchServer(server)
      request = started.client
    })()

    after(() => started?.stop())

    it('serves a hashed client bundle as immutable', async () => {
      await ready
      const response = await request('/__ruvyxa/client/app.abc123.js')
      assert.equal(response.status, 200)
      assert.equal(response.headers.get('content-type'), 'text/javascript; charset=utf-8')
      assert.equal(response.headers.get('cache-control'), 'public, max-age=31536000, immutable')
      assert.equal((await response.text()).trim(), 'export default 1')
    })

    it('serves a public asset with a revalidating lifetime', async () => {
      await ready
      const response = await request('/logo.png')
      assert.equal(response.status, 200)
      assert.equal(response.headers.get('content-type'), 'image/png')
      assert.equal(response.headers.get('cache-control'), 'public, max-age=3600, must-revalidate')
    })

    /**
     * `image.keepOriginal: false` publishes only the WebP, and the same markup
     * is served by `ruvyxa start` and by this program. A URL that resolves under
     * one and 404s under the other is a broken page that only appears after a
     * deploy.
     */
    it('answers a PNG URL with the WebP the build published', async () => {
      await ready
      const response = await request('/photo.png')
      assert.equal(response.status, 200)
      assert.equal(response.headers.get('content-type'), 'image/webp')
    })

    /**
     * The reverse of the case above, and the one `resolve_public_asset`
     * documents as load-bearing. `<Image>` rewrites every src to `webpUrl(src)`
     * unconditionally — it has no access to `image.optimize` — and
     * `image.optimize: false` publishes the source untouched with no `.webp`
     * beside it. Without this every `<Image>` on the page renders broken on
     * every self-hosted deployment of such a project, while `ruvyxa dev` and
     * `ruvyxa start` both resolve it.
     */
    it('answers a WebP URL with the source the build left untouched', async () => {
      await ready
      const response = await request('/source.webp')
      assert.equal(response.status, 200)
      assert.equal(response.headers.get('content-type'), 'image/png')
      assert.equal(await response.text(), 'not-really-a-png')
    })

    /**
     * Exactly one candidate, which is the Rust guard's rule. A first-hit loop
     * would answer this by array order and the two hosts would disagree about
     * the same publish directory.
     */
    it('refuses a WebP URL that two published sources could answer', async () => {
      await ready
      assert.equal((await request('/ambiguous.webp')).status, 404)
    })

    it('answers a byte range with exactly the requested bytes', async () => {
      await ready
      const response = await request('/clip.mp4', { headers: { range: 'bytes=2-5' } })
      assert.equal(response.status, 206)
      assert.equal(response.headers.get('content-range'), `bytes 2-5/${CLIP_BYTES.length}`)
      assert.equal(response.headers.get('content-length'), '4')
      assert.equal(await response.text(), '2345')
    })

    /**
     * `if-range` decides whether a resumed download may continue. This server
     * sends both an entity tag and a `last-modified`, so clients do send it,
     * and honouring the range unconditionally assembled bytes from two
     * different versions of a file into one corrupt result. Both validator
     * forms are answered, as `requested_range` answers them on the native host
     * serving the same `public/` directory.
     */
    it('continues a resumed download only while the file is the one it started', async () => {
      await ready
      const first = await request('/clip.mp4')
      const etag = first.headers.get('etag')
      const lastModified = first.headers.get('last-modified')
      assert.ok(etag, 'the response has to carry the validator a resume sends back')
      assert.ok(lastModified, 'and the date form of it')

      for (const validator of [etag, lastModified]) {
        const resumed = await request('/clip.mp4', {
          headers: { range: 'bytes=2-5', 'if-range': validator },
        })
        assert.equal(resumed.status, 206, `if-range: ${validator} must resume`)
        assert.equal(await resumed.text(), '2345')
      }

      for (const stale of ['"0000000000000000"', 'Thu, 01 Jan 1970 00:00:00 GMT']) {
        const restarted = await request('/clip.mp4', {
          headers: { range: 'bytes=2-5', 'if-range': stale },
        })
        assert.equal(
          restarted.status,
          200,
          `if-range: ${stale} names another version, so the whole file is the answer`,
        )
        assert.equal(await restarted.text(), CLIP_BYTES.toString())
      }
    })

    it('refuses a range past the end of the file', async () => {
      await ready
      const response = await request('/clip.mp4', { headers: { range: 'bytes=99-' } })
      assert.equal(response.status, 416)
      assert.equal(response.headers.get('content-range'), `bytes */${CLIP_BYTES.length}`)
      assert.equal(await response.text(), '')
    })

    /**
     * A HEAD answers with the headers the GET would have sent, `content-length`
     * included: a client that sizes a download from HEAD and then asks for
     * ranges has to be told the same length twice.
     */
    it('answers HEAD with the GET headers and no body', async () => {
      await ready
      const response = await request('/logo.png', { method: 'HEAD' })
      assert.equal(response.status, 200)
      assert.equal(response.headers.get('content-length'), '16')
      assert.equal(await response.text(), '')
    })

    it('routes a page through the handler', async () => {
      await ready
      const response = await request('/')
      assert.equal(response.status, 200)
      assert.match(await response.text(), /<title>home<\/title>/)
    })

    /**
     * The publish directory is the fallback for a path routing did not claim,
     * which is what makes a hand-written `public/*.html` reachable.
     */
    it('falls back to a published HTML file when routing finds nothing', async () => {
      await ready
      const response = await request('/fallback')
      assert.equal(response.status, 200)
      assert.match(await response.text(), /<title>fallback<\/title>/)
    })

    it('404s a path that neither routing nor the publish directory answers', async () => {
      await ready
      assert.equal((await request('/nothing-here')).status, 404)
      // Asset-shaped and missing: it must fall through rather than being
      // answered from some other file.
      assert.equal((await request('/missing.png')).status, 404)
    })

    /**
     * `publicDir` containment is enforced before the file system is touched, so
     * a traversal cannot reach a sibling of the publish directory even though
     * the server can read it.
     */
    it('refuses to serve a file outside the publish directory', async () => {
      await ready
      for (const attempt of ['/../secret.txt', '/%2e%2e/secret.txt', '/..%2fsecret.txt']) {
        const response = await request(attempt)
        assert.notEqual(
          await response.text(),
          'must never be served',
          `${attempt} escaped the publish directory`,
        )
      }
    })

    /**
     * `/__ruvyxa/health` is in this list because it is the response the two
     * fetch transports answered before routing and therefore outside the two
     * places they added the defaults — while the Node transport sets them on
     * `res` at the top of every request and the Axum host applies them as the
     * outermost layer over its whole router. Probing only `/logo.png` and `/`
     * is what let one file serve two different header policies depending on
     * which runtime picked it up.
     */
    it('applies the security defaults to both static files and rendered pages', async () => {
      await ready
      for (const pathname of ['/logo.png', '/', '/__ruvyxa/health']) {
        const response = await request(pathname)
        assert.equal(
          response.headers.get('x-content-type-options'),
          'nosniff',
          `missing on ${pathname}`,
        )
        assert.equal(response.headers.get('x-frame-options'), 'DENY', `missing on ${pathname}`)
      }
    })

    /**
     * `content-encoding` is not asserted here: an installed runtime is driven
     * through `fetch`, which decodes the body and removes the header before a
     * test can see it. What survives that is `Vary`, which is the part a shared
     * cache depends on — without it a proxy hands one client's gzip copy to a
     * client that cannot read it. The encoded bytes themselves are checked
     * against all three real runtimes by `scripts/smoke-runtime-adapter.mjs`.
     */
    it('declares Vary on a compressible response and not on a binary one', async () => {
      await ready
      const page = await request('/')
      assert.match(
        String(page.headers.get('vary') ?? ''),
        /accept-encoding/i,
        'a document must vary by Accept-Encoding',
      )

      const image = await request('/logo.png')
      assert.doesNotMatch(
        String(image.headers.get('vary') ?? ''),
        /accept-encoding/i,
        'a PNG is already compressed, so nothing about it varies by encoding',
      )
      assert.equal(image.headers.get('content-encoding'), null)
    })

    /**
     * A range describes byte offsets into the identity encoding. Compressing
     * underneath a `content-range` hands the client a window it cannot map back
     * to the bytes it asked for, and the file this uses is `text/javascript` —
     * compressible, so the exclusion is what keeps it out rather than its type.
     */
    it('never encodes a range response', async () => {
      await ready
      const response = await request('/__ruvyxa/client/app.abc123.js', {
        headers: { range: 'bytes=0-3' },
      })
      assert.equal(response.status, 206)
      assert.equal(response.headers.get('content-encoding'), null)
      assert.equal(response.headers.get('content-length'), '4')
    })

    /**
     * `cache-control: public, max-age=3600, must-revalidate` is a promise that
     * a revalidation will be answered, and without a validator this server had
     * nothing to answer it with: every image, font, and video came back in full
     * on every revalidation, while `ruvyxa start` answered the same file with an
     * ETag and a 304. Same project, same file, different bytes on the wire
     * depending on which server picked it up.
     */
    it('answers a revalidation of a public asset with 304', async () => {
      await ready
      const first = await request('/logo.png')
      assert.equal(first.status, 200)
      const etag = first.headers.get('etag')
      assert.ok(etag, 'a public asset must carry a validator to revalidate against')
      assert.ok(first.headers.get('last-modified'), 'and a date, for a client that sends neither')

      const revalidated = await request('/logo.png', { headers: { 'if-none-match': etag } })
      assert.equal(revalidated.status, 304)
      assert.equal(revalidated.headers.get('etag'), etag)
      // A `content-length` beside an empty body is a framing error the client
      // reads as a truncated response.
      assert.equal(revalidated.headers.get('content-length'), null)
      assert.equal(await revalidated.text(), '')
    })

    it('answers a revalidation by date when the client has no validator', async () => {
      await ready
      const first = await request('/logo.png')
      const modified = first.headers.get('last-modified') ?? ''
      const response = await request('/logo.png', { headers: { 'if-modified-since': modified } })
      assert.equal(response.status, 304)
    })

    it('sends the file when the validator names a version it no longer holds', async () => {
      await ready
      const response = await request('/logo.png', { headers: { 'if-none-match': '"stale"' } })
      assert.equal(response.status, 200)
      assert.equal(await response.text(), 'not-really-a-png')
    })

    /**
     * The validator is weak because the same file is served identity or gzipped
     * depending on what the client accepts. A shared cache that stored the gzip
     * copy under a strong ETag would hand it to a client that cannot read it.
     */
    it('validates a compressible asset weakly', async () => {
      await ready
      const response = await request('/__ruvyxa/client/app.abc123.js')
      assert.match(String(response.headers.get('etag') ?? ''), /^W\//)
    })

    /**
     * A server-sent-event stream reaches the client as it is produced.
     *
     * `COMPRESSIBLE_TYPE` begins `^(?:text\/`, which matches
     * `text/event-stream`, and the size floor is explicitly waived for a body
     * with no declared length — which is exactly what an SSE response is. Node
     * then inserts a default-flush gzip and Bun/Deno a `CompressionStream`, and
     * neither flushes per chunk, so `EventSource` received nothing until
     * roughly 16 KB had accumulated. The Axum host reaches the opposite
     * decision through tower-http's `DefaultPredicate`, so this is a silent
     * dev/prod divergence reachable from any application API route.
     *
     * The deadline is the assertion: a first chunk that arrives at all proves
     * nothing was buffering it.
     */
    it('never compresses a server-sent-event stream', async () => {
      await ready
      const response = await request('/events', { headers: { 'accept-encoding': 'gzip' } })
      assert.equal(response.status, 200)
      assert.equal(response.headers.get('content-type'), 'text/event-stream')
      assert.equal(response.headers.get('content-encoding'), null)
      assert.doesNotMatch(
        String(response.headers.get('vary') ?? ''),
        /accept-encoding/i,
        'a response that cannot be encoded must not advertise a variance it does not have',
      )

      const reader = response.body?.getReader()
      assert.ok(reader, 'a server-sent-event response must carry a body')
      const tooSlow = Symbol('the first event never arrived')
      let timer: ReturnType<typeof setTimeout> | undefined
      try {
        const first = await Promise.race([
          reader.read(),
          new Promise<typeof tooSlow>((resolve) => {
            timer = setTimeout(() => resolve(tooSlow), 4_000)
          }),
        ])
        assert.notEqual(
          first,
          tooSlow,
          'the first event must reach the client as it is produced, not once an encoder has filled',
        )
        const chunk = first as ReadableStreamReadResult<Uint8Array>
        assert.match(new TextDecoder().decode(chunk.value), /data: first/)
      } finally {
        clearTimeout(timer)
        await reader.cancel()
      }
    })

    /**
     * `Cache-Control: no-transform` is the header an application has to say
     * "do not re-encode this" with today, and neither host read it. The payload
     * is plainly compressible and well past the size floor, so the directive is
     * the only thing that can be keeping it as it was written.
     */
    it('honours no-transform on a compressible payload', async () => {
      await ready
      const response = await request('/plain', { headers: { 'accept-encoding': 'gzip' } })
      assert.equal(response.status, 200)
      assert.equal(response.headers.get('content-encoding'), null)
      assert.doesNotMatch(
        String(response.headers.get('vary') ?? ''),
        /accept-encoding/i,
        'a payload that will never be encoded does not vary by Accept-Encoding',
      )
      assert.equal((await response.text()).length, 4096)
    })
  })
}

/**
 * A connection that never finishes its request is retired.
 *
 * `headersTimeout` is the knob Node documents for this and the one the emitted
 * program has always set — and on Node 24 it does not fire. Measured against a
 * bare `node:http` server: a socket that writes a request line and stops is held
 * open at every value down to three seconds, with `requestTimeout` no better,
 * so each one costs a socket and a parser for as long as the caller cares to
 * hold it.
 *
 * Node only. Bun retires the same connection on its own (measured: twelve
 * seconds, unprompted) and Deno's server exposes nothing to bound it with, so
 * this is the one transport that both needed the guard and could carry it.
 */
describe('generated standalone server connection deadlines', () => {
  it('retires a connection that stalls halfway through its request', async () => {
    const server = stageDeployment('node')
    const started = await startProcessServer(process.execPath, [], server, {
      // The same value the program already computes for this, dialled down so
      // the test measures behaviour rather than patience.
      RUVYXA_KEEP_ALIVE_TIMEOUT: '800',
      RUVYXA_HEADERS_TIMEOUT: '1200',
    })
    try {
      // A request that has begun and will never end. `fetch` cannot express
      // this, which is why the socket is driven directly.
      const socket = netConnect(started.port, '127.0.0.1')
      const closed = new Promise<string>((resolve) => {
        const timer = setTimeout(() => resolve('still open'), 8_000)
        const settle = (how: string) => {
          clearTimeout(timer)
          resolve(how)
        }
        socket.on('connect', () => socket.write('GET / HTTP/1.1\r\nHost: localhost\r\n'))
        socket.on('close', () => settle('closed'))
        socket.on('error', () => settle('closed'))
      })
      assert.equal(
        await closed,
        'closed',
        'a half-sent request holds a socket and a parser until somebody closes it',
      )
      socket.destroy()

      // And the guard is only about that window: a complete request is still
      // answered on a connection the same deadline governs.
      const response = await started.client('/logo.png')
      assert.equal(response.status, 200)
    } finally {
      started.stop()
    }
  })
})

/**
 * A render that never settles is given up on.
 *
 * `ruvyxa start` bounds the same render through the worker pool's
 * `RUVYXA_WORKER_TIMEOUT_MS`, and a serverless adapter inherits its platform's
 * invocation limit. This host had neither, so one hung route held its
 * connection, its memory, and whatever it was waiting on for as long as the
 * process lived — a slow leak that ends as an out-of-memory kill with nothing
 * in the log.
 *
 * Every runtime, because the decision is shared: a Bun deployment that kept
 * serving a hung render while the Node one gave up would be the two disagreeing
 * about the same build.
 */
describe('generated standalone server render deadline', () => {
  for (const runtime of runtimes) {
    const launcher = executables[runtime]
    it(`answers 503 when a render never settles (${runtime}, ${launcher ? 'installed' : 'stubbed'})`, async () => {
      const server = stageDeployment(runtime, true, { hangingRoute: true })
      const started = launcher
        ? await startProcessServer(launcher.executable, launcher.args, server, {
            RUVYXA_RENDER_TIMEOUT: '700',
          })
        : await startFetchServer(server, { RUVYXA_RENDER_TIMEOUT: '700' })
      try {
        // Bounded well past the deadline under test: without the guard the
        // server never answers, and an unbounded wait would make that a hanging
        // test rather than a failing one.
        const response = await started.client('/', { signal: AbortSignal.timeout(6_000) })
        assert.equal(response.status, 503)
        // What a proxy reads to decide this is worth trying again, and what
        // keeps a shared cache from storing the giving-up as the page.
        assert.equal(response.headers.get('retry-after'), '1')
        assert.equal(response.headers.get('cache-control'), 'no-store')

        // The deadline belongs to one request. A server that answered 503 and
        // then stopped serving would have traded one hung route for all of them.
        const asset = await started.client('/logo.png')
        assert.equal(asset.status, 200)
      } finally {
        started.stop()
      }
    })
  }
})

/**
 * The health probe an orchestrator points at this process.
 *
 * Answered before routing and before admission, which is the part worth a test:
 * a probe that queues behind the renders it exists to report on says
 * "unhealthy" when the server is merely busy, and the orchestrator restarts a
 * process that was working. Driven here against a server whose every slot is
 * held by a render that never settles.
 */
describe('generated standalone server health endpoint', () => {
  for (const runtime of runtimes) {
    const launcher = executables[runtime]
    it(`answers the probe while every render slot is taken (${runtime}, ${launcher ? 'installed' : 'stubbed'})`, async () => {
      const server = stageDeployment(runtime, true, { hangingRoute: true })
      const limits = { RUVYXA_MAX_CONCURRENCY: '1', RUVYXA_MAX_QUEUE: '1' }
      const started = launcher
        ? await startProcessServer(launcher.executable, launcher.args, server, limits)
        : await startFetchServer(server, limits)
      try {
        const healthy = await started.client('/__ruvyxa/health', {
          signal: AbortSignal.timeout(6_000),
        })
        assert.equal(healthy.status, 200)
        assert.equal(healthy.headers.get('cache-control'), 'no-store')
        assert.deepEqual(await healthy.json(), { status: 'ok', host: runtime })

        // Fill both slots with renders that will never finish, then ask again.
        const parked = Array.from({ length: 2 }, () =>
          started.client('/', { signal: AbortSignal.timeout(15_000) }).catch(() => null),
        )
        await new Promise((resolve) => setTimeout(resolve, 400))

        const underLoad = await started.client('/__ruvyxa/health', {
          signal: AbortSignal.timeout(6_000),
        })
        assert.equal(
          underLoad.status,
          200,
          'a busy server is not an unhealthy one, and a probe that queues cannot tell them apart',
        )

        // A probe often asks with HEAD; the status has to mean the same thing.
        const head = await started.client('/__ruvyxa/health', {
          method: 'HEAD',
          signal: AbortSignal.timeout(6_000),
        })
        assert.equal(head.status, 200)
        assert.equal(await head.text(), '')

        // The path exists; this verb does not. A 404 would say the endpoint is
        // absent, which is what the native host answers 405 to avoid — and a
        // probe misconfigured to POST should be told which verb to use rather
        // than that the endpoint was never deployed.
        const wrongVerb = await started.client('/__ruvyxa/health', {
          method: 'POST',
          signal: AbortSignal.timeout(6_000),
        })
        assert.equal(wrongVerb.status, 405)
        assert.equal(wrongVerb.headers.get('allow'), 'GET, HEAD')

        void parked
      } finally {
        started.stop()
      }
    })
  }

  /**
   * The readiness half: once a drain has begun the probe has to say so.
   *
   * An orchestrator still routing to a process that has stopped accepting sends
   * it work it can only refuse, and this is the only thing that tells it in
   * time. Runs everywhere — see `startProcessServer` for how the signal is
   * raised on a platform that cannot deliver one.
   */
  it('reports itself draining once a shutdown signal has arrived', async () => {
    const server = stageDeployment('node')
    const started = await startProcessServer(
      process.execPath,
      [],
      server,
      {
        RUVYXA_SHUTDOWN_GRACE: '10000',
        // Stated rather than left to the default, so this measures the window
        // and not the number the default happens to hold. Long enough that
        // the probe lands inside it, short enough to wait out.
        RUVYXA_DRAIN_DELAY: '3000',
      },
      { drainable: true },
    )
    try {
      assert.equal((await started.client('/__ruvyxa/health')).status, 200)

      started.drain()
      await new Promise((resolve) => setTimeout(resolve, 300))

      // A new connection, which is what a readiness probe opens. Answering it
      // is the whole point: closing the socket on the signal made this a
      // connection refusal, so nothing could ever read the draining status.
      const draining = await started.client('/__ruvyxa/health', {
        signal: AbortSignal.timeout(6_000),
      })
      assert.equal(draining.status, 503)
      assert.equal(draining.headers.get('retry-after'), '1')
      assert.deepEqual(await draining.json(), { status: 'draining', host: 'node' })

      // The window has to end, or a deploy never replaces the process. Waited
      // for rather than assumed, because a drain that only ever begins looks
      // exactly like this one until it does not finish.
      const deadline = Date.now() + 12_000
      while (!started.output().includes('shutdown complete')) {
        assert.ok(Date.now() < deadline, `the drain never finished:\n${started.output()}`)
        await new Promise((resolve) => setTimeout(resolve, 100))
      }
    } finally {
      started.stop()
    }
  })

  /**
   * The window is one decision; only how a transport stops listening differs.
   *
   * The drain above runs the Node process, because that is the transport whose
   * close is hand-built. Bun and Deno hand theirs to the runtime, and a fix
   * applied to the one that is measured and not to the two that are not is the
   * shape this repository keeps paying for.
   *
   * Presence, not wiring: what the window does is proved by the process test.
   * This exists so removing it from a transport cannot be silent.
   */
  it('holds every transport behind the drain window before it stops accepting', () => {
    for (const runtime of ['node', 'bun', 'deno'] as const) {
      const source = standaloneServerSource({ runtime })
      // Node defers its own `server.close`; the other two await the delay before
      // handing the close to the runtime.
      const deferral =
        runtime === 'node' ? '}, DRAIN_DELAY_MS);' : 'setTimeout(resolve, DRAIN_DELAY_MS)'
      assert.ok(
        source.includes(deferral),
        `${runtime}: the socket must not close until the drain window has passed`,
      )
      assert.ok(
        source.includes('RUVYXA_DRAIN_DELAY'),
        `${runtime}: the window must stay configurable`,
      )
    }
  })
})

/**
 * Every line the process writes is one record a collector can read.
 *
 * The point of the format is not that it looks tidy: it is that the escaping
 * stops being something a future edit has to remember. `JSON.stringify` cannot
 * emit a raw newline, so a value a caller supplied cannot become a second record
 * — which is a thing that happened, see the log-injection test over
 * `serverless-handler.mjs`.
 *
 * Node only. What is under test is the shared program text, and the three
 * transports write through the same writer.
 */
describe('generated standalone server structured logging', () => {
  it('writes one JSON record per line when asked to', async () => {
    const server = stageDeployment('node')
    const started = await startProcessServer(process.execPath, [], server, {
      RUVYXA_LOG_FORMAT: 'json',
      RUVYXA_MAX_CONCURRENCY: '2',
    })
    try {
      // A request that is logged, so the request record is in the output too.
      await started.client('/logo.png')
      await new Promise((resolve) => setTimeout(resolve, 200))

      const lines = started
        .output()
        .split('\n')
        .map((line) => line.trim())
        .filter((line) => line !== '')
      assert.ok(lines.length > 0, 'the server must have said something')
      for (const line of lines) {
        // The whole claim: no line needs a parser other than JSON.
        const record = JSON.parse(line) as { level?: string; msg?: string }
        assert.ok(record.level, `every record carries a level: ${line}`)
        assert.ok(record.msg, `every record carries a message: ${line}`)
      }
      const listening = lines
        .map((line) => JSON.parse(line) as { msg?: string; url?: string })
        .find((record) => record.msg === 'listening on')
      assert.ok(listening?.url, 'the readiness record carries where it is listening')
    } finally {
      started.stop()
    }
  })

  it('keeps the human shape by default', async () => {
    const server = stageDeployment('node')
    const started = await startProcessServer(process.execPath, [], server)
    try {
      const first = started.output().split('\n')[0]
      assert.match(first, /^\[ruvyxa] /, first)
      assert.throws(() => JSON.parse(first), 'the default must not be JSON')
    } finally {
      started.stop()
    }
  })
})

/**
 * Metrics, and the token that is the only reason they can be public at all.
 *
 * These are the numbers the health probe deliberately withholds — concurrency,
 * queue depth, refusals — so the interesting assertions are the negative ones:
 * unset, the path must not exist; set, a caller without the token must not get
 * a single number out of it.
 */
describe('generated standalone server metrics endpoint', () => {
  const TOKEN = 'metrics-token-for-the-test'

  for (const runtime of runtimes) {
    const launcher = executables[runtime]
    const start = (extraEnv: Record<string, string>) =>
      launcher
        ? startProcessServer(launcher.executable, launcher.args, stageDeployment(runtime), extraEnv)
        : startFetchServer(stageDeployment(runtime), extraEnv)

    it(`does not advertise the path when no token is configured (${runtime})`, async () => {
      const started = await start({})
      try {
        // 404 rather than 401: a deployment that never turned metrics on should
        // not tell a stranger that the endpoint is there to be attacked.
        const response = await started.client('/__ruvyxa/metrics')
        assert.equal(response.status, 404)
      } finally {
        started.stop()
      }
    })

    it(`refuses a scrape without the token (${runtime})`, async () => {
      const started = await start({ RUVYXA_METRICS_TOKEN: TOKEN })
      try {
        const anonymous = await started.client('/__ruvyxa/metrics')
        assert.equal(anonymous.status, 401)
        assert.equal(anonymous.headers.get('www-authenticate'), 'Bearer')
        assert.doesNotMatch(await anonymous.text(), /ruvyxa_/, 'a refusal must carry no numbers')

        const wrong = await started.client('/__ruvyxa/metrics', {
          headers: { authorization: `Bearer ${TOKEN}x` },
        })
        assert.equal(wrong.status, 401)
        // Same length, different bytes: the comparison must not accept a
        // prefix, which is the shape a `startsWith` or a truncated compare has.
        const sameLength = await started.client('/__ruvyxa/metrics', {
          headers: { authorization: `Bearer ${'x'.repeat(TOKEN.length)}` },
        })
        assert.equal(sameLength.status, 401)
      } finally {
        started.stop()
      }
    })

    it(`reports the admission numbers to a scrape that holds the token (${runtime})`, async () => {
      const started = await start({
        RUVYXA_METRICS_TOKEN: TOKEN,
        RUVYXA_MAX_CONCURRENCY: '3',
        RUVYXA_MAX_QUEUE: '7',
      })
      try {
        const response = await started.client('/__ruvyxa/metrics', {
          headers: { authorization: `Bearer ${TOKEN}` },
        })
        assert.equal(response.status, 200)
        assert.match(String(response.headers.get('content-type')), /^text\/plain; version=0\.0\.4/)
        assert.equal(response.headers.get('cache-control'), 'no-store')

        const body = await response.text()
        // The configured limits, read back — a scrape that reported defaults
        // would be describing a server other than this one.
        assert.match(body, /^ruvyxa_renders_max_concurrent 3$/m, body)
        assert.match(body, /^ruvyxa_renders_max_queued 7$/m, body)
        assert.match(body, /^ruvyxa_renders_active \d+$/m, body)
        assert.match(body, /^ruvyxa_renders_rejected_total \d+$/m, body)
        assert.match(body, new RegExp(`^ruvyxa_build_info\\{runtime="${runtime}"\\} 1$`, 'm'), body)
        // Prometheus needs the type line to read a counter as one.
        assert.match(body, /^# TYPE ruvyxa_renders_rejected_total counter$/m, body)

        // Answered before routing, which is also before the only two places
        // the fetch transports added the defaults. A scrape endpoint that a
        // browser can frame or content-type-sniff is not a different class of
        // response from the pages beside it.
        assert.equal(response.headers.get('x-content-type-options'), 'nosniff')
        assert.equal(response.headers.get('x-frame-options'), 'DENY')
      } finally {
        started.stop()
      }
    })

    /**
     * The path exists and this verb does not, which is what `/__ruvyxa/health`
     * already answers 405 to say. Falling through to routing answered 404 —
     * "no such endpoint" — to an operator who had pointed a scraper at it with
     * the wrong method, and a 404 is the one answer that sends them looking at
     * their deployment rather than at their scrape config.
     */
    it(`answers a non-read verb on a configured scrape path with 405 (${runtime})`, async () => {
      const started = await start({ RUVYXA_METRICS_TOKEN: TOKEN })
      try {
        const response = await started.client('/__ruvyxa/metrics', {
          method: 'POST',
          headers: { authorization: `Bearer ${TOKEN}` },
        })
        assert.equal(response.status, 405)
        assert.equal(response.headers.get('allow'), 'GET, HEAD')
        assert.equal(response.headers.get('x-content-type-options'), 'nosniff')
      } finally {
        started.stop()
      }
    })

    /**
     * ...and only when it is configured. An unset token means the path does not
     * exist as far as anyone asking can tell, and a 405 would say it does.
     */
    it(`still hides an unconfigured scrape path from a non-read verb (${runtime})`, async () => {
      const started = await start({})
      try {
        const response = await started.client('/__ruvyxa/metrics', { method: 'POST' })
        assert.equal(response.status, 404)
      } finally {
        started.stop()
      }
    })
  }
})

/**
 * The response a runtime produces when the fetch handler itself throws.
 *
 * Bun's `error` and Deno's `onError` sit outside `handleRequest` entirely, so
 * neither of the two places those transports add the security defaults can
 * reach them — and the Node transport's equivalent is a `catch` inside a
 * request whose `res` already carries them. Driven through the stand-in servers
 * in this file rather than through an installed runtime, because the hook is
 * reached only by a throw the emitted program has no way to stage from outside.
 */
/**
 * `isrCache: 'tmp'` and what the directory it picks is allowed to be shared
 * with.
 *
 * The compute bundle an immutable host deploys cannot be written to, so ISR
 * refreshes go to the host's temporary directory instead — read *before* the
 * bundled prerender output, which is what makes a stale entry there win. A
 * fixed name under `os.tmpdir()` is the same directory for every Ruvyxa
 * deployment on the host and for every build of the same deployment.
 *
 * Run on the Node transport, because the cache directory is decided in the
 * shared half of the program that all three transports run unchanged — the
 * three differ in how a request reaches it and how a file becomes a body, and
 * this is neither.
 */
describe('generated standalone server temporary ISR cache', () => {
  /** The directory every build of every deployment used to share. */
  const cacheRoot = path.join(tmpdir(), 'ruvyxa-isr-cache')

  function stageTmpIsrDeployment(server: string, buildId: string): void {
    mkdirSync(server, { recursive: true })
    mkdirSync(path.resolve(server, '..', 'public'), { recursive: true })
    for (const file of HANDLER_RUNTIME_FILES) {
      cpSync(repoPath('packages/ruvyxa/runtime', file), path.join(server, file))
    }
    writeFileSync(
      path.join(server, 'index.mjs'),
      standaloneServerSource({ runtime: 'node', isrCache: 'tmp', buildId, runtimePolicy: {} }),
      'utf8',
    )
    writeFileSync(
      path.join(server, 'manifest.mjs'),
      'export default ' +
        JSON.stringify({
          routes: [
            {
              id: 'isr',
              path: '/isr',
              kind: 'page',
              file: 'isr.tsx',
              // Long enough that nothing under test is ever answered by the
              // staleness clock instead of by the cache.
              render: { strategy: 'isr', revalidate: 3600 },
            },
          ],
        }) +
        '\n',
      'utf8',
    )
    writeFileSync(
      path.join(server, 'route-modules.mjs'),
      [
        // Which build rendered it, and how many times this process has. The
        // first says whether a document came from the other deployment; the
        // second says whether it came from a cache at all.
        'let renders = 0',
        'export async function loadRouteModule() {',
        '  return {',
        '    render: async () => {',
        '      renders += 1',
        '      return `<!doctype html><title>isr</title><p>${process.env.RUVYXA_TEST_MARK}-${renders}</p>`',
        '    },',
        '  }',
        '}',
        'export async function loadActionModule() { return {} }',
        'export const applyPluginHttp = undefined',
        '',
      ].join('\n'),
      'utf8',
    )
  }

  it('does not let one build read the documents another build wrote', async () => {
    const before = new Set(existsSync(cacheRoot) ? readdirSync(cacheRoot) : [])
    const root = mkdtempSync(path.join(tmpdir(), 'ruvyxa-isr-tmp-'))
    const server = path.join(root, 'server')

    // The first build, deployed and warmed.
    stageTmpIsrDeployment(server, 'buildidaaaaaaaaaaaaaaaaaaaaaaaaa')
    let started = await startProcessServer(process.execPath, [], server, {
      RUVYXA_TEST_MARK: 'first',
    })
    try {
      const cold = await started.client('/isr')
      assert.equal(cold.status, 200)
      assert.match(await cold.text(), /first-1/)

      // The positive control, and the only thing that makes the assertion
      // below mean anything: the refresh really was written to the temporary
      // directory and really is read back from it. Without a store this would
      // be `first-2`.
      const warm = await started.client('/isr')
      assert.equal(warm.headers.get('x-ruvyxa-isr'), 'HIT')
      assert.match(await warm.text(), /first-1/)
    } finally {
      started.stop()
    }
    // The process has to be gone before its directory is redeployed over.
    await new Promise((resolve) => setTimeout(resolve, 300))

    // The second build, deployed *over the first* — same path, same host, same
    // temporary directory. This is the redeploy of an Amplify compute bundle,
    // which is the deployment shape `isrCache: 'tmp'` exists for.
    stageTmpIsrDeployment(server, 'buildidbbbbbbbbbbbbbbbbbbbbbbbbb')
    started = await startProcessServer(process.execPath, [], server, {
      RUVYXA_TEST_MARK: 'second',
    })
    try {
      const response = await started.client('/isr')
      const body = await response.text()
      assert.match(
        body,
        /second-1/,
        'a redeploy must render its own page, not serve the previous build from a shared directory',
      )
      // Said twice on purpose: the stale document is what a visitor would have
      // been served, and its `<script src>` names client chunks this build no
      // longer publishes.
      assert.doesNotMatch(body, /first/, body)
    } finally {
      started.stop()
    }

    const created = (existsSync(cacheRoot) ? readdirSync(cacheRoot) : []).filter(
      (entry) => !before.has(entry),
    )
    assert.equal(
      created.length,
      2,
      `two builds must not share one cache directory (created: ${created.join(', ')})`,
    )
    for (const entry of created) {
      rmSync(path.join(cacheRoot, entry), { recursive: true, force: true })
    }
    rmSync(root, { recursive: true, force: true })
  })
})

describe('generated standalone server transport error hook', () => {
  for (const runtime of ['bun', 'deno'] as const) {
    it(`carries the security defaults on the ${runtime} transport's own 500`, async () => {
      const started = await startFetchServer(stageDeployment(runtime))
      const hook = started.errorHook
      assert.ok(hook, `the ${runtime} transport registered no error hook`)
      const response = await hook(new Error('render exploded'))
      assert.equal(response.status, 500)
      assert.equal(response.headers.get('content-type'), 'text/plain; charset=utf-8')
      assert.equal(response.headers.get('x-content-type-options'), 'nosniff')
      assert.equal(response.headers.get('x-frame-options'), 'DENY')
    })
  }
})

/**
 * More requests than the machine can render are refused, not accepted.
 *
 * Nothing bounded this: every request that arrived got a render started for it,
 * so a burst larger than the machine became a heap holding every in-flight
 * render at once. The failure that produces is not a slow server — it is an
 * out-of-memory kill that takes down the requests already nearly finished along
 * with the ones that caused it.
 *
 * Driven with a route that never settles, because that is the only way to hold
 * every slot at once deterministically: a render that finishes would free its
 * slot before the next request could be refused, and the test would pass on an
 * unbounded server too.
 */
describe('generated standalone server admission control', () => {
  for (const runtime of runtimes) {
    const launcher = executables[runtime]
    it(`refuses a render it has no capacity for (${runtime}, ${launcher ? 'installed' : 'stubbed'})`, async () => {
      const server = stageDeployment(runtime, true, { hangingRoute: true })
      const limits = {
        RUVYXA_MAX_CONCURRENCY: '2',
        RUVYXA_MAX_QUEUE: '2',
        // Long enough that the deadline cannot be what answers here: this is
        // measuring admission, not the render timeout beside it.
        RUVYXA_RENDER_TIMEOUT: '30000',
      }
      const started = launcher
        ? await startProcessServer(launcher.executable, launcher.args, server, limits)
        : await startFetchServer(server, limits)
      try {
        // Two fill the slots and two fill the queue; none of them will ever
        // settle, so every later arrival has nowhere to go.
        const parked = Array.from({ length: 4 }, () =>
          started.client('/', { signal: AbortSignal.timeout(20_000) }).catch(() => null),
        )
        // Give them time to be admitted before measuring the overflow. Without
        // this the fifth request could win the race for a slot and the
        // assertion below would be about scheduling rather than about capacity.
        await new Promise((resolve) => setTimeout(resolve, 500))

        const refused = await started.client('/', { signal: AbortSignal.timeout(6_000) })
        assert.equal(refused.status, 503)
        assert.equal(refused.headers.get('retry-after'), '1')

        // Static files never entered admission, so a page that is failing does
        // not take its own stylesheet down with it.
        const asset = await started.client('/logo.png', { signal: AbortSignal.timeout(6_000) })
        assert.equal(asset.status, 200)

        void parked
      } finally {
        started.stop()
      }
    })
  }
})

/**
 * A request body is read with a render slot in hand, never before one.
 *
 * `requestInit.body = await readRequestBody(req)` ran before `handleAdmitted`,
 * so `MAX_CONCURRENT_RENDERS`/`MAX_QUEUED_RENDERS` — added specifically to stop
 * a burst larger than the machine becoming a heap holding every in-flight
 * render at once — did not bound it. Bun and Deno never buffer, so the Node
 * deployment of one artifact could be pushed to an out-of-memory kill by
 * concurrent uploads the other two refuse.
 *
 * Node only, because it is the only transport that buffers at all: the other
 * two hand `createHandler` the `Request` the runtime gave them.
 *
 * The two statuses are what makes this measurable. A server that reads first
 * hits the transport cap and answers 413; a server that admits first has
 * nowhere to put the request and answers 503 without having read a byte.
 */
describe('generated standalone server request bodies', () => {
  const UPLOAD = Buffer.alloc(256 * 1024, 'a')
  const API_LIMIT = 64 * 1024

  it('refuses an upload it has no capacity for before reading it (node)', async () => {
    const server = stageDeployment('node', true, { apiLimit: API_LIMIT, hangingRoute: true })
    const started = await startProcessServer(process.execPath, [], server, {
      RUVYXA_MAX_CONCURRENCY: '1',
      RUVYXA_MAX_QUEUE: '1',
      // Long enough that the deadline cannot be what answers here.
      RUVYXA_RENDER_TIMEOUT: '30000',
    })
    try {
      // One fills the slot and one fills the queue; neither will ever settle.
      const parked = Array.from({ length: 2 }, () =>
        started.client('/', { signal: AbortSignal.timeout(20_000) }).catch(() => null),
      )
      await new Promise((resolve) => setTimeout(resolve, 500))

      const response = await started.client('/whoami', {
        method: 'POST',
        body: UPLOAD,
        headers: { 'content-type': 'application/octet-stream' },
        signal: AbortSignal.timeout(10_000),
      })
      assert.equal(
        response.status,
        503,
        'a 413 here means the transport buffered the whole upload before asking whether it had anywhere to put it',
      )
      assert.equal(response.headers.get('retry-after'), '1')

      void parked
    } finally {
      started.stop()
    }
  })

  /**
   * And an upload past the project's own limit is still refused. Admission
   * comes first now, so a server with capacity has to reach the same 413 it
   * always did — the slot is taken, the request is refused against
   * `security.apiLimit`, and the slot is given back.
   *
   * Which half answers is deliberately not asserted: the transport bounds what
   * one request may allocate and the handler bounds what each endpoint may
   * accept, and a caller can only see that an oversized upload to an API route
   * is refused either way.
   */
  it('still refuses an upload past the API limit when it has capacity (node)', async () => {
    const server = stageDeployment('node', true, { apiLimit: API_LIMIT })
    const started = await startProcessServer(process.execPath, [], server)
    try {
      const response = await started.client('/whoami', {
        method: 'POST',
        body: UPLOAD,
        headers: { 'content-type': 'application/octet-stream' },
        signal: AbortSignal.timeout(10_000),
      })
      assert.equal(response.status, 413)

      // And the slot came back: a server that leaked one per oversized upload
      // would stop answering after its concurrency was spent.
      const after = await started.client('/', { signal: AbortSignal.timeout(10_000) })
      assert.equal(after.status, 200)
    } finally {
      started.stop()
    }
  })

  /**
   * A server-function call larger than `security.apiLimit` still reaches the
   * endpoint that is allowed to accept it.
   *
   * The transport cap was `security.apiLimit` alone, but that is the bound on a
   * project's *own* API routes — the framework's policy is per endpoint, and
   * `/__ruvyxa/rsc` is bounded by `RSC_ACTION_BODY_LIMIT` instead. A project
   * that lowered `apiLimit` therefore shrank the transport below an endpoint
   * limit the handler enforces itself, and a legitimate call sized between the
   * two was answered 413 by the transport before the endpoint ever ran. The
   * same deployed artifact accepted that call under Bun and Deno, which never
   * buffer, and refused it under Node.
   *
   * 501 is the answer this staged app owes a well-formed call: the route
   * renders, but declares no `'use server'` function. Reaching it at all is the
   * proof — the body was read, the endpoint ran, and nothing along the way
   * measured it against `apiLimit`.
   */
  it('lets a server-function call between the API and RSC limits through (node)', async () => {
    const server = stageDeployment('node', true, { apiLimit: API_LIMIT })
    const started = await startProcessServer(process.execPath, [], server)
    try {
      const response = await started.client('/__ruvyxa/rsc?path=%2F', {
        method: 'POST',
        // Above `apiLimit`, below `RSC_ACTION_BODY_LIMIT`.
        body: Buffer.alloc(2 * 1024 * 1024, 'a'),
        headers: {
          'content-type': 'text/plain;charset=UTF-8',
          'x-ruvyxa-rsc': '1',
          'x-ruvyxa-action': 'home#submit',
          // What the browser sends on a server-function call, and what the
          // endpoint's own origin gate requires: without it the call is
          // refused 403 for being unprovably same-origin, which is a different
          // refusal than the one under test.
          origin: `http://127.0.0.1:${started.port}`,
        },
        signal: AbortSignal.timeout(20_000),
      })
      assert.notEqual(
        response.status,
        413,
        'the transport cap must come from the largest limit any endpoint may allow, not from `apiLimit`',
      )
      assert.equal(response.status, 501)
    } finally {
      started.stop()
    }
  })

  /**
   * The cap is still a cap. It exists to bound what one buffered request can
   * allocate, so a body past every endpoint limit must be refused during the
   * read rather than after it — deriving the number from the endpoint policy
   * must not become removing it.
   */
  it('refuses an upload past every endpoint limit (node)', async () => {
    const server = stageDeployment('node', true, { apiLimit: API_LIMIT })
    const started = await startProcessServer(process.execPath, [], server)
    try {
      const response = await started.client('/whoami', {
        method: 'POST',
        // Past `RSC_ACTION_BODY_LIMIT`, which is the largest of them.
        body: Buffer.alloc(5 * 1024 * 1024, 'a'),
        headers: { 'content-type': 'application/octet-stream' },
        signal: AbortSignal.timeout(20_000),
      })
      assert.equal(response.status, 413)

      const after = await started.client('/', { signal: AbortSignal.timeout(10_000) })
      assert.equal(after.status, 200)
    } finally {
      started.stop()
    }
  })
})

/**
 * `security.headers: false` is a project's decision and every runtime has to
 * honour it, or the same build answers differently depending on which server
 * picked it up.
 */
describe('generated standalone server security policy', () => {
  for (const runtime of runtimes) {
    it(`omits the security defaults when the build turned them off (${runtime})`, async () => {
      const server = stageDeployment(runtime, false)
      const launcher = executables[runtime]
      const started = launcher
        ? await startProcessServer(launcher.executable, launcher.args, server)
        : await startFetchServer(server)
      const response = await started.client('/logo.png')
      assert.equal(response.status, 200)
      assert.equal(response.headers.get('x-content-type-options'), null)
      started.stop()
    })
  }
})

/**
 * Who a request belongs to, decided where the peer actually is.
 *
 * `createHandler` scans `X-Forwarded-For` from the right and has no peer to
 * weigh it against — a deployed function does not have one, which is why an
 * adapter for a platform with an ingress declares `clientIpHeaders` instead.
 * This program is the third case: it *is* a socket server, all three transports
 * have the peer available, and none of them read it. So every self-hosted
 * deployment reachable without a header-overwriting proxy in front of it —
 * the README's Docker/PM2/systemd case, any container with a published port —
 * believed whatever the caller typed, and one client rotating the header
 * collected a fresh bucket per request from the built-in `rate` middleware, the
 * server-action rate limiter, and the replay quota at once. `ruvyxa start` has
 * always gated this on the transport peer; this is the alignment.
 *
 * The Rust twin is `forwarded_identity_is_ignored_when_the_peer_is_not_trusted`
 * in `crates/ruvyxa_middleware/src/client_ip.rs`.
 */
describe('generated standalone server forwarded identity', () => {
  /** The forwarded headers the handler was actually handed. */
  async function forwardedHeadersSeen(response: Response): Promise<[string, string][]> {
    // Read once: a body is consumed by whichever of `text()` and `json()` gets
    // to it first, so an assertion that reports the body cannot also parse it.
    const body = await response.text()
    assert.equal(response.status, 200, body)
    const pairs = JSON.parse(body) as [string, string][]
    return pairs.filter(([name]) => name === 'x-forwarded-for' || name === 'x-real-ip')
  }

  const forwarded = {
    'x-forwarded-for': '203.0.113.7',
    'x-real-ip': '203.0.113.7',
  }

  // Bun and Deno only: a peer is the second argument to their fetch handler, so
  // a stub is the only way to reach a non-loopback one. A real socket in a test
  // comes from 127.0.0.1, and loopback is trusted without configuration — which
  // is exactly why a naive test of this cannot fail.
  for (const runtime of ['bun', 'deno'] as const) {
    it(`drops a forwarded identity from an untrusted peer (${runtime})`, async () => {
      const server = stageDeployment(runtime, true, { trustedProxyIps: ['10.0.0.0/8'] })
      const started = await startFetchServer(server)
      const seen = await forwardedHeadersSeen(
        await started.peerClient('/whoami', '198.51.100.4', { headers: forwarded }),
      )
      assert.deepEqual(
        seen,
        [],
        'a peer that is not a configured proxy must not be able to rename itself',
      )
      started.stop()
    })

    it(`believes a peer inside the configured prefix (${runtime})`, async () => {
      const server = stageDeployment(runtime, true, { trustedProxyIps: ['10.0.0.0/8'] })
      const started = await startFetchServer(server)
      const seen = await forwardedHeadersSeen(
        await started.peerClient('/whoami', '10.0.0.9', { headers: forwarded }),
      )
      assert.deepEqual(seen, [
        ['x-forwarded-for', '203.0.113.7'],
        ['x-real-ip', '203.0.113.7'],
      ])
      started.stop()
    })

    it(`believes a loopback peer without configuration (${runtime})`, async () => {
      // A proxy terminating on the same host is the ordinary deployment, and
      // the native host trusts it unconfigured. Diverging here would break
      // every Docker Compose and systemd deployment that has one.
      const server = stageDeployment(runtime, true)
      const started = await startFetchServer(server)
      const seen = await forwardedHeadersSeen(
        await started.peerClient('/whoami', '127.0.0.1', { headers: forwarded }),
      )
      assert.deepEqual(seen, [
        ['x-forwarded-for', '203.0.113.7'],
        ['x-real-ip', '203.0.113.7'],
      ])
      started.stop()
    })

    it(`drops a forwarded identity when the runtime reports no peer (${runtime})`, async () => {
      // A closed socket, or a transport with no address at all. Nothing that is
      // not an address can be loopback or inside a configured prefix, so the
      // answer has to be the same one an unknown peer gets — the failure that
      // reads as "trusted" is the one worth writing a case for.
      const server = stageDeployment(runtime, true, { trustedProxyIps: ['10.0.0.0/8'] })
      const started = await startFetchServer(server)
      const response = await started.client('/whoami', { headers: forwarded })
      assert.deepEqual(await forwardedHeadersSeen(response), [])
      started.stop()
    })
  }

  it('keeps a loopback peer trusted on the node transport', async () => {
    // The Node transport is only reachable as a real process, and a real socket
    // in a test comes from 127.0.0.1 — so what this can prove is the half that
    // has to keep working: the peer is read, and a same-host proxy is still
    // believed. The strip itself is covered above, and the source assertion
    // below is what holds all three transports to reading a peer at all.
    const server = stageDeployment('node', true, { trustedProxyIps: ['10.0.0.0/8'] })
    const started = await startProcessServer(process.execPath, [], server)
    try {
      const response = await started.client('/whoami', { headers: forwarded })
      assert.equal(response.status, 200)
      const pairs = (await response.json()) as [string, string][]
      assert.ok(
        pairs.some(([name, value]) => name === 'x-forwarded-for' && value === '203.0.113.7'),
        'a proxy terminating on the same host must still be believed',
      )
    } finally {
      started.stop()
    }
  })

  it('makes the trust decision in every transport, where the peer is', () => {
    // Behaviour above cannot see a transport that stopped asking, because the
    // one runtime that can be driven as a real process only ever has a trusted
    // peer. This is what fails when a fourth transport is added and forgets.
    const peerExpressions = {
      node: 'req.socket?.remoteAddress',
      bun: 'server?.requestIP?.(request)?.address',
      deno: 'info?.remoteAddr?.hostname',
    } as const
    for (const runtime of runtimes) {
      const source = standaloneServerSource({ runtime, runtimePolicy: {} })
      assert.ok(
        source.includes(peerExpressions[runtime]),
        `the ${runtime} transport no longer reads the peer the socket arrived on`,
      )
      assert.ok(
        source.includes('peerMayStateClientIdentity('),
        `the ${runtime} transport no longer weighs the peer before the handler sees a forwarded header`,
      )
    }
  })
})
