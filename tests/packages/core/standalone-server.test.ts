import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'

import { repoPath } from '../../repo-root.js'
import { standaloneServerSource } from '../../../packages/@ruvyxa/core/dist/standalone-server.js'

const generated = standaloneServerSource({ runtimePolicy: {} })

/**
 * The node, bun, deno, aws, railway, and render adapters all deploy the source
 * this module returns. It is assembled as a template string, so nothing in the
 * normal build checks it: `tsc` validates the *template*, not the JavaScript
 * that comes out of it, and the first execution happens on the user's host
 * after a deploy. These tests are the only place the emitted program is checked
 * before it ships.
 */
describe('generated standalone server', () => {
  /**
   * A single unescaped backtick anywhere in the template — including inside a
   * comment — closes the string early and emits a file that cannot parse. That
   * failure surfaces at deploy time on the user's machine, not here, so the
   * emitted program is parsed as the program it will actually be.
   */
  it('emits syntactically valid JavaScript for every runtime', () => {
    const directory = mkdtempSync(path.join(tmpdir(), 'ruvyxa-standalone-'))
    for (const runtime of ['node', 'bun', 'deno'] as const) {
      const file = path.join(directory, `${runtime}.mjs`)
      writeFileSync(file, standaloneServerSource({ runtime, runtimePolicy: {} }), 'utf8')
      // `isrCache: 'tmp'` emits an import and a cache-directory block the
      // default never reaches, so the default alone would have parsed a
      // program no adapter deploys.
      const temporaryCache = path.join(directory, `${runtime}-tmp-isr.mjs`)
      writeFileSync(
        temporaryCache,
        standaloneServerSource({ runtime, isrCache: 'tmp', buildId: 'abc123', runtimePolicy: {} }),
        'utf8',
      )
      execFileSync(process.execPath, ['--check', temporaryCache], { stdio: 'pipe' })
      // Node parses all three: the Bun and Deno programs use no syntax it does
      // not have, and the runtime-specific part of each is an API call, not a
      // dialect. A program that did need one would fail here rather than after
      // a deploy.
      execFileSync(process.execPath, ['--check', file], { stdio: 'pipe' })
    }
  })

  /**
   * A `<video>` in a deployed app is served by this program, and by
   * `ruvyxa start` during development. Without ranges, a seek restarts the
   * download from zero and a strict player refuses the resource outright — so
   * a clip that scrubs locally would not scrub after a deploy.
   *
   * The parser itself is not asserted here: it is imported from
   * `serverless-handler.mjs`, and both it and the Rust server answer
   * `tests/fixtures/byte-range-conformance.json`. What this checks is that this
   * server reaches for that parser rather than growing a third copy of the rule.
   */
  it('answers byte-range requests for static files', () => {
    assert.ok(
      generated.includes('parseByteRange'),
      'ranges must be decided by the shared parser, not reimplemented here',
    )
    assert.match(
      generated,
      /createReadStream\(plan\.file, \{ start: plan\.partial\.start, end: plan\.partial\.end \}\)/,
      'only the requested bytes may be read; re-reading the file to reach a late seek is the cost ranges exist to avoid',
    )
  })

  /**
   * Statuses, headers, and byte windows are decided once, in
   * `staticResponsePlan`, and a transport only sends what it is handed. A
   * second copy of any of that is how two runtimes come to disagree about a
   * cache lifetime or a content type with nothing to catch it — the emitted
   * behaviour itself is checked for all three in
   * `standalone-server-conformance.test.ts`.
   */
  it('decides a static response once for every runtime', () => {
    for (const runtime of ['node', 'bun', 'deno'] as const) {
      const source = standaloneServerSource({ runtime, runtimePolicy: {} })
      const definitions = source.match(/function staticResponsePlan\(/g) ?? []
      assert.equal(definitions.length, 1, `${runtime} must decide static responses in one place`)
      assert.equal(
        (source.match(/'accept-ranges'/g) ?? []).length,
        2,
        `${runtime} advertises ranges from the plan alone, once for a hit and once for a 416`,
      )
      assert.doesNotMatch(
        source,
        /MIME_TYPES\[[\s\S]{0,120}\]\s*\?\?[\s\S]{0,80}MIME_TYPES\[/,
        `${runtime} must look up a content type once`,
      )
    }
  })

  /**
   * The body is read with a render slot in hand, never before one.
   *
   * The Node transport is the only one that buffers — `node:http` gives it a
   * stream and `new Request` needs bytes — and it buffered ahead of admission,
   * so the controller that exists to stop a burst becoming a heap did not bound
   * the one path that allocates. The behaviour is measured in
   * `standalone-server-conformance.test.ts`; this is what fails if the two
   * lines are ever swapped back.
   */
  it('admits a request before reading its body', () => {
    const node = standaloneServerSource({ runtime: 'node', runtimePolicy: {} })
    const admitted = node.indexOf('handleAdmitted(async () =>')
    const read = node.indexOf('await readRequestBody(req)')
    assert.ok(admitted > 0, 'the node transport must run the handler through admission')
    assert.ok(read > 0, 'the node transport still has to buffer a body for `new Request`')
    assert.ok(
      read > admitted,
      'the body must be read inside the admitted work, not before the slot is asked for',
    )
  })

  /**
   * The buffered-body cap is the framework's own maximum, not the project's
   * `security.apiLimit`.
   *
   * The policy lives in `requestBodyPolicy` in `serverless-handler.mjs` and is
   * per endpoint: `/__ruvyxa/rsc` is bounded by `RSC_ACTION_BODY_LIMIT` however
   * small a project's `apiLimit` is. This transport buffers before the handler
   * runs, so its number has to cover every endpoint the handler may accept —
   * anything lower refuses a legitimate call the endpoint never got to see.
   *
   * The 4 MiB itself cannot be imported: `serverless-handler.mjs` is copied
   * into the function bundle and declares it locally, and this source is a
   * template string. So the two are compared here, which is the only place the
   * pair is visible at all.
   */
  it('bounds a buffered body by the largest limit any endpoint may allow', () => {
    const handler = readFileSync(repoPath('packages/ruvyxa/runtime/serverless-handler.mjs'), 'utf8')
    const declared = /\bconst RSC_ACTION_BODY_LIMIT = ([^\n]+)/.exec(handler)
    assert.ok(declared, 'the handler must still declare the server-function body limit')
    const expression = declared[1].trim().replace(/;$/, '')
    assert.ok(
      generated.includes(`const RSC_ACTION_BODY_LIMIT = ${expression};`),
      `the emitted server must bound its read by the same limit the endpoint applies (${expression})`,
    )
    const cap = generated.slice(generated.indexOf('const REQUEST_BODY_LIMIT ='))
    assert.match(
      cap.slice(0, cap.indexOf(';') + 1),
      /Math\.max\(/,
      '`security.apiLimit` alone would put the transport under an endpoint limit the handler enforces',
    )
  })

  /**
   * The allow-list is a prefix regex, and `^text\/` swallows the one text type
   * that must never be buffered. The refusal has to sit inside
   * `isCompressibleType` rather than `compressionFor`, so the
   * `Vary: accept-encoding` derived from the same predicate is suppressed with
   * it: a response that cannot be encoded must not advertise a variance it does
   * not have.
   */
  it('refuses to encode the payloads that must never be buffered', () => {
    for (const runtime of ['node', 'bun', 'deno'] as const) {
      const source = standaloneServerSource({ runtime, runtimePolicy: {} })
      assert.ok(
        source.includes('text\\/event-stream'),
        `${runtime}: a server-sent-event stream must never be encoded`,
      )
      assert.ok(
        source.includes('application\\/grpc'),
        `${runtime}: excluded by tower-http's DefaultPredicate, so excluded here`,
      )
      assert.ok(
        source.includes('no-transform'),
        `${runtime}: the header an application says this with must be read`,
      )
      const predicate = source.slice(source.indexOf('function isCompressibleType('))
      assert.match(
        predicate.slice(0, predicate.indexOf('\n}')),
        /NON_COMPRESSIBLE_TYPE/,
        `${runtime}: the refusal must gate Vary as well as the encoder`,
      )
    }
  })

  /**
   * Every container platform stops a deploy by sending SIGTERM and killing the
   * process shortly after. Node's default action is to exit immediately, which
   * drops every response still being written — so a rolling deploy shows users
   * connection resets rather than being invisible.
   */
  it('drains in-flight requests on a shutdown signal', () => {
    assert.match(generated, /process\.on\(signal/, 'must install signal handlers')
    assert.ok(generated.includes("'SIGTERM'"), 'SIGTERM is what orchestrators send')
    assert.ok(generated.includes("'SIGINT'"), 'SIGINT is what a local operator sends')
    assert.ok(
      generated.includes('server.close('),
      'must stop accepting and wait for in-flight work',
    )
    assert.ok(
      generated.includes('closeIdleConnections'),
      'idle keep-alive sockets would otherwise hold the drain open for a full keep-alive window',
    )
  })

  /**
   * The 502 this prevents is the classic one: the proxy keeps a pooled socket
   * it believes is alive, the origin has already started closing it, and the
   * request that lands on it fails. It appears only under load, only in
   * production, and only intermittently.
   */
  it('keeps connections alive longer than a load balancer will', () => {
    const keepAlive = /RUVYXA_KEEP_ALIVE_TIMEOUT', ([\d_]+)\)/.exec(generated)
    assert.ok(keepAlive, 'keep-alive timeout must be set explicitly, not left at the Node default')
    const milliseconds = Number(keepAlive[1].replaceAll('_', ''))
    // AWS ALB idles at 60s and is the tightest of the common managed proxies;
    // anything at or below that lets the origin retire the connection first.
    assert.ok(
      milliseconds > 60_000,
      `keep-alive must exceed a 60s proxy idle window, got ${milliseconds}ms`,
    )
    assert.ok(
      generated.includes('server.headersTimeout'),
      'headersTimeout must be raised with it or Node times out a connection it would have kept',
    )
  })

  /**
   * A rejection thrown outside a request's own try/catch terminates the process
   * by default, taking every concurrent request with it. One bad route must not
   * be able to do that. An uncaught exception is treated differently on purpose:
   * the process state is no longer trustworthy, so it drains and exits non-zero
   * for the supervisor to replace.
   */
  it('survives an unhandled rejection but replaces itself after an uncaught exception', () => {
    assert.ok(generated.includes("process.on('unhandledRejection'"), 'must be handled')
    assert.ok(generated.includes("process.on('uncaughtException'"), 'must be handled')
    const uncaught = generated.slice(generated.indexOf("process.on('uncaughtException'"))
    assert.match(
      uncaught,
      /shutdown\('uncaught exception', 1\)/,
      'an uncaught exception must drain and exit non-zero, not keep serving',
    )
  })

  /**
   * Both pipes commit their status and headers before the body flows, so a
   * later failure can only end the connection. Left unhandled, the stream's
   * `error` event is fatal to the whole process — an aborted download would
   * take out every other request being served.
   */
  it('contains stream failures and client disconnects', () => {
    assert.ok(
      generated.includes("file.on('error'"),
      'a static read that fails mid-response must not be fatal',
    )
    assert.ok(
      generated.includes("body.on('error'"),
      'a response stream that fails mid-response must not be fatal',
    )
    assert.ok(
      generated.includes("res.on('close', () => file.destroy())"),
      'a client that leaves must stop the file read',
    )
    assert.ok(
      generated.includes("res.on('close', () => body.destroy())"),
      'a client that leaves must stop the render still producing for it',
    )
  })

  /**
   * `isrCache: 'tmp'` writes rendered documents into the host's shared
   * temporary directory, which is the only writable place an immutable server
   * bundle has. A fixed name there is the same directory for every Ruvyxa
   * deployment on the host and for every build of the same deployment — and it
   * is read *before* the bundled prerender output, so a stale document wins.
   * The name has to come from something that changes when the deployment does.
   */
  it('namespaces the temporary ISR cache to one build of one deployment', () => {
    const source = standaloneServerSource({ isrCache: 'tmp', buildId: 'abc123', runtimePolicy: {} })
    // Narrowed before it is asserted on, so a failure prints the one
    // declaration under test rather than the whole emitted program.
    const directory = /const isrCacheDir = ([\s\S]*?);\n/.exec(source)?.[1] ?? ''
    assert.doesNotMatch(
      directory,
      /^path\.join\(\s*os\.tmpdir\(\),\s*'ruvyxa-isr-cache',?\s*\)$/,
      'the shared temporary directory may not be entered by a name every deployment shares',
    )
    assert.ok(
      directory.includes('abc123'),
      `the build the bundle was emitted from has to reach the directory name: ${directory}`,
    )
    assert.ok(
      directory.includes('here'),
      `and so does where it was deployed, or two deployments share it: ${directory}`,
    )
    assert.match(
      source,
      /mkdirSync\(isrCacheDir, \{ recursive: true, mode: 0o700 \}\)/,
      'the directory is created in a world-writable parent, so it has to be created owner-only',
    )

    // Two builds of the same deployment, and the same build deployed twice:
    // the first pair must differ, or a redeploy reads the previous build's
    // documents; the second must not, or an in-place restart loses its cache
    // for no reason.
    const other = standaloneServerSource({ isrCache: 'tmp', buildId: 'def456', runtimePolicy: {} })
    assert.notEqual(source, other)
    assert.equal(
      source,
      standaloneServerSource({ isrCache: 'tmp', buildId: 'abc123', runtimePolicy: {} }),
    )

    // And nothing changes for the bundled cache, which is per-deployment by
    // construction: it is a directory inside the bundle.
    assert.match(
      standaloneServerSource({ buildId: 'abc123', runtimePolicy: {} }),
      /const isrCacheDir = prerenderDir;/,
    )
  })
})

/**
 * The ISR temporary-cache directory is derived in one place.
 *
 * Four hosts write to it — the three serverless adapters and this server — and
 * each used to spell the derivation itself. That is how `CORE-10` was fixed in
 * exactly one of the four: the finding named `isrCache: 'tmp'`, that copy was
 * corrected, and Vercel, Netlify and Firebase went on joining a fixed
 * `ruvyxa-isr-cache` onto `os.tmpdir()` — the same directory for every Ruvyxa
 * deployment on the host and for every previous build of this one, read
 * *before* the bundled prerender output. Correcting them meant writing the same
 * derivation a third and fourth time, which is the state a rule is in just
 * before it drifts again.
 *
 * A source scan rather than a behavioural assertion, because the failure is a
 * *second declaration site* and no output can show one exists.
 */
describe('the ISR temporary cache has one derivation', () => {
  it('is spelled in exactly one source file', () => {
    const tracked = execFileSync(
      'git',
      ['ls-files', '--cached', '--others', '--exclude-standard', '--', '*.ts', '*.mjs'],
      { cwd: repoPath('.'), encoding: 'utf8' },
    )
      .split('\n')
      .map((file) => file.trim())
      .filter((file) => file && !file.includes('node_modules/') && !file.startsWith('tests/'))

    const declarations = tracked.filter((file) => {
      const source = readFileSync(repoPath(file), 'utf8')
      // The join itself, however it is broken across lines: the literal name
      // appearing near `tmpdir()` is what a second derivation looks like.
      return /tmpdir\(\)[\s\S]{0,40}'ruvyxa-isr-cache'/.test(source)
    })

    assert.deepEqual(
      declarations,
      ['packages/@ruvyxa/core/src/utils.ts'],
      'every host must reach the directory through isrTemporaryCacheDirSource',
    )
  })
})
