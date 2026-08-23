import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import {
  createReadStream,
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  writeFileSync,
} from 'node:fs'
import { createServer } from 'node:net'
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

/**
 * Stage the directory an adapter's `function` artifact produces.
 *
 * The real `serverless-handler.mjs` and the modules it imports are copied in,
 * exactly as `materializeFunction` copies them, so what is under test is the
 * emitted server against the handler it will actually be deployed with — not
 * against a stand-in that agrees with it today. Only the route modules are
 * stubbed, because compiling a project is not what this is measuring.
 */
function stageDeployment(runtime: 'node' | 'bun' | 'deno', securityHeaders = true): string {
  const root = mkdtempSync(path.join(tmpdir(), `ruvyxa-standalone-${runtime}-`))
  const server = path.join(root, 'server')
  const publicDir = path.join(root, 'public')
  mkdirSync(server, { recursive: true })
  mkdirSync(path.join(publicDir, '__ruvyxa', 'client'), { recursive: true })

  for (const file of HANDLER_RUNTIME_FILES) {
    cpSync(repoPath('packages/ruvyxa/runtime', file), path.join(server, file))
  }

  writeFileSync(
    path.join(server, 'index.mjs'),
    standaloneServerSource({
      runtime,
      runtimePolicy: securityHeaders ? {} : { security: { headers: false } },
    }),
    'utf8',
  )
  writeFileSync(
    path.join(server, 'manifest.mjs'),
    'export default ' +
      JSON.stringify({
        routes: [
          { id: 'home', path: '/', kind: 'page', file: 'home.tsx', render: { strategy: 'ssr' } },
        ],
      }) +
      '\n',
    'utf8',
  )
  writeFileSync(
    path.join(server, 'route-modules.mjs'),
    [
      'export async function loadRouteModule(routeId) {',
      '  return { render: async ({ path: pathname }) =>',
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
): Promise<{ client: Client; stop: () => void }> {
  const port = await freePort()
  const child = spawn(executable, [...args, path.join(server, 'index.mjs')], {
    cwd: server,
    env: { ...process.env, PORT: String(port), HOST: '127.0.0.1' },
    stdio: ['ignore', 'pipe', 'pipe'],
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
const globals = globalThis as MutableGlobal
globals.Bun = {
  // Synchronous like the real one, and a Blob like the real one: a `Bun.file`
  // is accepted by `new Response` and answers `.slice`.
  file: (file: string) => new Blob([readFileSync(file)]),
  serve: (options: { fetch: (request: Request) => Promise<Response> }) => {
    registeredHandler = options.fetch
    return { stop: () => {} }
  },
}
globals.Deno = {
  open: async (file: string) => ({ readable: Readable.toWeb(createReadStream(file)) }),
  serve: (
    options: { onListen?: (address: { hostname: string; port: number }) => void },
    fetchHandler: (request: Request) => Promise<Response>,
  ) => {
    options.onListen?.({ hostname: '127.0.0.1', port: 0 })
    registeredHandler = fetchHandler
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
function startFetchServer(server: string): Promise<{ client: Client; stop: () => void }> {
  const next = loading.then(async () => {
    // Signals and error events do not share an overload on `process`, and this
    // needs to treat them alike.
    const emitter = process as unknown as {
      listeners(event: string): (() => void)[]
      off(event: string, listener: () => void): void
    }
    const events = ['uncaughtException', 'unhandledRejection', 'SIGTERM', 'SIGINT']
    const owned = new Map(events.map((event) => [event, new Set(emitter.listeners(event))]))
    registeredHandler = undefined
    try {
      await import(pathToFileURL(path.join(server, 'index.mjs')).href)
    } finally {
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
    const handler = registeredHandler as ((request: Request) => Promise<Response>) | undefined
    assert.ok(handler, `${server} never registered a fetch handler`)
    return {
      // The handler these runtimes register takes a `Request`, which is the
      // whole point of the transport: there is nothing between the socket and
      // it.
      client: (pathname: string, init?: RequestInit) =>
        handler(new Request(`http://127.0.0.1${pathname}`, init)),
      stop: () => {},
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

    it('answers a byte range with exactly the requested bytes', async () => {
      await ready
      const response = await request('/clip.mp4', { headers: { range: 'bytes=2-5' } })
      assert.equal(response.status, 206)
      assert.equal(response.headers.get('content-range'), `bytes 2-5/${CLIP_BYTES.length}`)
      assert.equal(response.headers.get('content-length'), '4')
      assert.equal(await response.text(), '2345')
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

    it('applies the security defaults to both static files and rendered pages', async () => {
      await ready
      for (const pathname of ['/logo.png', '/']) {
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
  })
}

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
