/**
 * The HMR client the dev server injects, held by running it.
 *
 * `hmr_client_script()` in `crates/ruvyxa_dev_server/src/html_document.rs`
 * emits the JavaScript that every `ruvyxa dev` browser executes, and it was
 * gated by nine `assert!(script.contains(...))` calls. That is the arrangement
 * `AGENTS.md` names as passing on exactly the bugs the class produces: the
 * substrings survive a refactor that inverts a comparison, drops an `await`, or
 * emits a script that does not parse at all, and none of the nine would notice.
 *
 * So this suite parses the literal out of the Rust source and executes it in a
 * `vm` context against stand-in browser globals, then asks it questions a text
 * match cannot: does a stale sequence stay ignored, does a CSS update avoid the
 * reload, does an apply that a newer sequence overtook abandon itself, does a
 * refresh boundary that throws still fall back to a reload.
 *
 * The stand-ins are deliberately the smallest thing the script actually
 * touches, the way `entry-prelude-parity.test.mjs` gives the preludes just
 * enough React. Anything more would be testing the harness.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import vm from 'node:vm'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const htmlDocumentRs = readFileSync(
  path.join(workspaceRoot, 'crates/ruvyxa_dev_server/src/html_document.rs'),
  'utf8',
)

/**
 * The wire contract both HMR halves answer to.
 *
 * `watcher.rs` replayed this fixture from the producing side and nothing read it
 * from the consuming side, so the client's copy of the same three decisions --
 * which protocol name and version it accepts, which `(type, kind)` pairs it
 * knows, and what it does with a stale sequence -- lived here as hand-written
 * literals. A `protocolVersion` bump would have kept `watcher.rs` green while
 * every dev browser silently full-reloaded on every save, and this suite would
 * have kept building its own `protocolVersion: 1` message and passing.
 */
const WIRE = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/hmr-contract.json'), 'utf8'),
)

const TRACE_ID = '0123456789abcdef0123456789abcdef'

/** The fixture entry for one declared event, so a rename fails loudly here. */
function wireMessage(event) {
  const declared = WIRE.messages.find((entry) => entry.event === event)
  assert.ok(declared, `tests/fixtures/hmr-contract.json declares no "${event}" message`)
  return { type: declared.type, kind: declared.kind }
}

/**
 * Read the client out of the Rust raw literal that emits it.
 *
 * Asserts the literal is exactly one `<script>` element rather than stripping
 * whatever it finds: if the emitter starts wrapping the client differently,
 * this should fail here with a clear reason rather than execute a fragment.
 */
function hmrClientSource() {
  const match = /fn hmr_client_script\(\) -> &'static str \{\s*r#"([\s\S]*?)"#\s*\}/.exec(
    htmlDocumentRs,
  )
  assert.ok(match, 'hmr_client_script() not found in html_document.rs')
  const body = /^<script>([\s\S]*)<\/script>$/.exec(match[1].trim())
  assert.ok(body, 'expected hmr_client_script() to emit exactly one <script> element')
  return body[1]
}

const CLIENT_SOURCE = hmrClientSource()

/** A `<link rel=stylesheet>` stand-in: the script only reads and writes `href`. */
function stylesheetLink(href) {
  return { href }
}

/**
 * Build a context, run the client in it, and hand back the levers a test needs.
 *
 * `deliver` resolves once the message handler has fully settled, so a test can
 * assert on what the client did rather than on a timer.
 */
function runClient({
  protocol = 'http:',
  href = 'http://localhost:3000/docs',
  pathname = '/docs',
  links = [],
  inlineStyle = null,
  nextStyle = null,
  routePattern,
  refresh,
  fetchImpl,
} = {}) {
  const reloads = []
  const fetches = []
  const consoleErrors = []
  const marks = []
  const created = []
  let socketUrl = null
  let messageHandler = null

  const defaultFetch = async (url, init) => {
    fetches.push({ url, init })
    return {
      ok: true,
      text: async () => '<html></html>',
    }
  }

  const context = {
    console: { error: (...args) => consoleErrors.push(args) },
    performance: { mark: (name) => marks.push(name) },
    URL,
    JSON,
    Number,
    Array,
    Promise,
    String,
    location: {
      protocol,
      host: 'localhost:3000',
      href,
      pathname,
      reload: () => reloads.push(true),
    },
    WebSocket: class {
      constructor(url) {
        socketUrl = url
      }
      addEventListener(type, handler) {
        if (type === 'message') messageHandler = handler
      }
    },
    document: {
      querySelectorAll: (selector) => (selector === 'link[rel="stylesheet"][href]' ? links : []),
      querySelector: (selector) => (selector === 'style[data-ruvyxa-css]' ? inlineStyle : null),
      // Recorded rather than omitted. The client must never patch a boundary by
      // injecting a script, and a spy proves that by not being called; leaving
      // `createElement` off the stand-in would prove it only by crashing, which
      // reads as a harness gap rather than as the invariant it is.
      createElement: (tag) => {
        created.push(tag)
        return {}
      },
    },
    DOMParser: class {
      parseFromString() {
        return {
          querySelector: (selector) => (selector === 'style[data-ruvyxa-css]' ? nextStyle : null),
        }
      }
    },
    fetch: (url, init) => {
      fetches.push({ url, init })
      return fetchImpl ? fetchImpl(url, init) : defaultFetch(url, init)
    },
  }
  context.globalThis = context
  if (routePattern !== undefined) context.__RUVYXA_ROUTE_PATTERN__ = routePattern
  if (refresh !== undefined) context.__RUVYXA_HMR_REFRESH__ = refresh

  // Executing the source is itself the parse check the nine substring
  // assertions never made.
  vm.runInNewContext(CLIENT_SOURCE, context, { filename: 'hmr-client.js' })

  assert.ok(messageHandler, 'the client did not register a message listener')

  return {
    get socketUrl() {
      return socketUrl
    },
    reloads,
    fetches,
    consoleErrors,
    marks,
    created,
    deliver: (message) =>
      messageHandler({ data: typeof message === 'string' ? message : JSON.stringify(message) }),
  }
}

/** A well-formed message; individual tests override only what they are about. */
function message(overrides = {}) {
  return {
    protocol: WIRE.protocol,
    protocolVersion: WIRE.protocolVersion,
    traceId: TRACE_ID,
    sequence: 1,
    ...wireMessage('css'),
    affectedRoutes: [],
    traceAck: false,
    ...overrides,
  }
}

describe('the emitted HMR client', () => {
  it('connects to the HMR endpoint over the page protocol', () => {
    assert.equal(
      runClient().socketUrl,
      'ws://localhost:3000/__ruvyxa/hmr',
      'a plain page should open an insecure socket',
    )
    assert.equal(
      runClient({ protocol: 'https:' }).socketUrl,
      'wss://localhost:3000/__ruvyxa/hmr',
      'a secure page must not open an insecure socket',
    )
  })

  it('reloads on a payload it cannot trust', async () => {
    assert.equal(WIRE.fallback, 'reload', 'the fixture must still name reload as the fallback')
    const cases = {
      'malformed JSON': 'not json at all',
      'another protocol': message({ protocol: 'vite.hmr' }),
      'another protocol version': message({ protocolVersion: WIRE.protocolVersion + 1 }),
      'a malformed trace id': message({ traceId: 'nope' }),
      'a fractional sequence': message({ sequence: 1.5 }),
    }

    for (const [reason, payload] of Object.entries(cases)) {
      const client = runClient()
      await client.deliver(payload)
      assert.equal(client.reloads.length, 1, `${reason} should force a reload`)
    }
  })

  it('recognises every message the wire contract declares', async () => {
    // Read the fixture's message table *inwards*: each declared event names a
    // behaviour here, and an event with no entry fails rather than being
    // skipped. Without this, a producer taught a new `kind` kept `watcher.rs`
    // green while the client dropped through to its final `reload()` -- the
    // fallback is correct for an unknown message and wrong for a known one, and
    // the two are indistinguishable from the outside.
    const behaviours = {
      // A CSS update patches the link in place. An unrecognised message could
      // only reload, so a rewritten href is the discriminating observation.
      css: async () => {
        const links = [stylesheetLink('http://localhost:3000/app.css')]
        const client = runClient({ links })
        await client.deliver(message({ sequence: 1, ...wireMessage('css') }))
        assert.match(links[0].href, /__ruvyxa_hmr=1/, 'a css update must patch the stylesheet')
        assert.equal(client.reloads.length, 0)
      },
      // Consulting the refresh boundary at all is the discriminator: an
      // unrecognised message reloads without asking.
      client: async () => {
        const client = runClient({ refresh: async () => true })
        await client.deliver(message({ sequence: 1, ...wireMessage('client') }))
        assert.equal(client.reloads.length, 0, 'an accepted boundary must keep the page')
      },
      // Route scoping is the discriminator: an unrecognised message reloads
      // regardless of which route changed.
      server: async () => {
        const client = runClient({ routePattern: '/docs' })
        await client.deliver(
          message({ sequence: 1, ...wireMessage('server'), affectedRoutes: ['/blog'] }),
        )
        assert.equal(client.reloads.length, 0, 'another route changing must not reload this one')
      },
      // A restart shares the unknown-message fallback by design, so the claim
      // here is only that it reloads and patches nothing.
      structural: async () => {
        const links = [stylesheetLink('http://localhost:3000/app.css')]
        const client = runClient({ links })
        await client.deliver(message({ sequence: 1, ...wireMessage('structural') }))
        assert.equal(client.reloads.length, 1, 'a structural change must reload')
        assert.doesNotMatch(links[0].href, /__ruvyxa_hmr/, 'and must not patch anything first')
      },
      // Reporting without reloading is the discriminator.
      failure: async () => {
        const client = runClient()
        await client.deliver(message({ sequence: 1, ...wireMessage('failure'), fullReload: false }))
        assert.equal(client.consoleErrors.length, 1, 'issues must be reported')
        assert.equal(client.reloads.length, 0, 'issues alone must not reload')
      },
    }

    assert.deepEqual(
      WIRE.messages.map((entry) => entry.event).sort(),
      Object.keys(behaviours).sort(),
      'every message in tests/fixtures/hmr-contract.json needs a behaviour here',
    )
    for (const [event, check] of Object.entries(behaviours)) {
      await check().catch((error) => {
        error.message = `${event}: ${error.message}`
        throw error
      })
    }
  })

  it('ignores a sequence it has already applied', async () => {
    assert.equal(
      WIRE.stalePolicy,
      'reject-sequence-less-than-or-equal-to-last-applied',
      'this test encodes the fixture stale policy; a new policy needs a new test',
    )
    const links = [stylesheetLink('http://localhost:3000/app.css')]
    const client = runClient({ links })

    await client.deliver(message({ sequence: 4 }))
    const afterFirst = links[0].href
    assert.match(afterFirst, /__ruvyxa_hmr=4/)

    // Equal and lower sequences are both stale. A `<` where the client means
    // `<=` would let the equal one through, and no substring assertion sees it.
    await client.deliver(message({ sequence: 4 }))
    await client.deliver(message({ sequence: 3 }))

    assert.equal(links[0].href, afterFirst, 'a stale sequence must not re-apply CSS')
    assert.equal(client.reloads.length, 0, 'a stale sequence must not reload')
  })

  it('rewrites stylesheet links without reloading', async () => {
    const links = [
      stylesheetLink('http://localhost:3000/app.css'),
      stylesheetLink('http://localhost:3000/theme.css?v=1'),
    ]
    const client = runClient({ links })

    await client.deliver(message({ sequence: 7 }))

    assert.match(links[0].href, /app\.css\?__ruvyxa_hmr=7$/)
    assert.match(links[1].href, /theme\.css\?v=1&__ruvyxa_hmr=7$/, 'existing query must survive')
    assert.equal(client.reloads.length, 0, 'a CSS update is the case that must not reload')
  })

  it('swaps collected inline CSS without reloading', async () => {
    const inlineStyle = { textContent: 'body{color:red}' }
    const client = runClient({
      inlineStyle,
      nextStyle: { textContent: 'body{color:blue}' },
    })

    await client.deliver(message({ sequence: 2 }))

    assert.equal(inlineStyle.textContent, 'body{color:blue}')
    assert.equal(client.reloads.length, 0)
  })

  it('reloads when a CSS update finds nothing it can patch', async () => {
    // No stylesheet links and no collected style block: the client cannot
    // apply anything, so correctness has to fall back to a reload.
    const client = runClient()

    await client.deliver(message({ sequence: 2 }))

    assert.equal(client.reloads.length, 1)
  })

  it('abandons a CSS apply that a newer sequence overtook', async () => {
    let releaseFetch
    const pending = new Promise((resolve) => {
      releaseFetch = () => resolve({ ok: true, text: async () => '<html></html>' })
    })
    const inlineStyle = { textContent: 'body{color:red}' }
    const client = runClient({
      inlineStyle,
      nextStyle: { textContent: 'body{color:blue}' },
      routePattern: '/docs',
      fetchImpl: () => pending,
    })

    // Sequence 2 starts applying and parks on the fetch.
    const first = client.deliver(message({ sequence: 2 }))
    // Sequence 3 arrives meanwhile. A server-route update for another route
    // returns without doing any work of its own, but it still moves
    // `lastSequence`, which is what sequence 2 must notice when it wakes.
    await client.deliver(message({ sequence: 3, kind: 'server-route', affectedRoutes: ['/other'] }))
    releaseFetch()
    await first

    assert.equal(
      inlineStyle.textContent,
      'body{color:red}',
      'the overtaken apply must not write stale CSS over a newer state',
    )
    assert.equal(client.reloads.length, 1, 'the overtaken apply falls back to a reload')
  })

  it('acknowledges a trace only when the message asks for one', async () => {
    const quiet = runClient({ links: [stylesheetLink('http://localhost:3000/a.css')] })
    await quiet.deliver(message({ sequence: 1, traceAck: false }))
    assert.deepEqual(
      quiet.fetches.filter((call) => String(call.url).includes('/__ruvyxa/trace')),
      [],
      'traceAck false must not report anything',
    )

    const acking = runClient({ links: [stylesheetLink('http://localhost:3000/a.css')] })
    await acking.deliver(message({ sequence: 1, traceAck: true }))
    const ack = acking.fetches.find((call) => String(call.url).includes('/__ruvyxa/trace'))
    assert.ok(ack, 'traceAck true must report the trace')
    assert.equal(ack.init.method, 'POST')
    assert.deepEqual(JSON.parse(ack.init.body), { traceId: TRACE_ID })
    assert.match(acking.marks[0], new RegExp(`ruvyxa:hmr:${TRACE_ID}:received`))
  })

  it('reloads on issues only when the message says so', async () => {
    const soft = runClient()
    await soft.deliver(message({ sequence: 1, type: 'issues', fullReload: false }))
    assert.equal(soft.reloads.length, 0, 'issues alone must not reload')
    assert.equal(soft.consoleErrors.length, 1, 'issues must still be reported')

    const hard = runClient()
    await hard.deliver(message({ sequence: 1, type: 'issues', fullReload: true }))
    assert.equal(hard.reloads.length, 1)
  })

  it('ignores a server-route update aimed at a different route', async () => {
    const other = runClient({ routePattern: '/docs' })
    await other.deliver(message({ sequence: 1, kind: 'server-route', affectedRoutes: ['/blog'] }))
    assert.equal(other.reloads.length, 0, 'another route changing must not reload this one')

    const mine = runClient({ routePattern: '/docs' })
    await mine.deliver(message({ sequence: 1, kind: 'server-route', affectedRoutes: ['/docs'] }))
    assert.equal(mine.reloads.length, 1, 'this route changing falls back to a reload')
  })

  it('falls back to the document pathname when no route pattern is published', async () => {
    const client = runClient({ pathname: '/docs' })
    await client.deliver(message({ sequence: 1, kind: 'server-route', affectedRoutes: ['/blog'] }))
    assert.equal(client.reloads.length, 0)
  })

  it('keeps the page when a client refresh boundary accepts the update', async () => {
    const accepted = runClient({ refresh: async () => true })
    await accepted.deliver(message({ sequence: 1, kind: 'client-boundary' }))
    assert.equal(accepted.reloads.length, 0, 'an accepted refresh must not reload')

    const declined = runClient({ refresh: async () => false })
    await declined.deliver(message({ sequence: 1, kind: 'client-boundary' }))
    assert.equal(declined.reloads.length, 1, 'a declined refresh falls back to a reload')
  })

  it('never patches a boundary by injecting a script', async () => {
    // The client deliberately falls back to a reload rather than hot-patching a
    // client boundary it cannot prove accepted the update -- "correctness wins",
    // as the emitter says. Script injection is the shortcut that would quietly
    // replace that, so no update of any kind may create an element.
    for (const kind of ['css', 'client-boundary', 'server-route']) {
      const client = runClient({ refresh: async () => false, routePattern: '/docs' })
      await client.deliver(message({ sequence: 1, kind, affectedRoutes: ['/docs'] }))
      assert.deepEqual(client.created, [], `a ${kind} update must not create an element`)
    }
  })

  it('reloads when a client refresh boundary throws', async () => {
    const client = runClient({
      refresh: async () => {
        throw new Error('boundary exploded')
      },
    })

    await client.deliver(message({ sequence: 1, kind: 'client-boundary' }))

    assert.equal(client.reloads.length, 1, 'a thrown boundary must not strand the page')
    assert.equal(client.consoleErrors.length, 1, 'and must say why')
  })
})
