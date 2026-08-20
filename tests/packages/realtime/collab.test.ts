import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { collab } from '../../../packages/@ruvyxa/realtime/dist/index.js'
import { createCollabClient } from '../../../packages/@ruvyxa/realtime/dist/collab.js'

class FakeSocket {
  readyState = 1
  sent: string[] = []
  closed = false
  readonly url: string
  private listeners = new Map<string, Array<(event: any) => void>>()

  constructor(url: string) {
    this.url = url
  }

  addEventListener(type: string, listener: (event: any) => void) {
    const values = this.listeners.get(type) ?? []
    values.push(listener)
    this.listeners.set(type, values)
  }

  send(data: string) {
    this.sent.push(data)
  }

  close() {
    this.closed = true
  }

  emit(type: string, event: any = {}) {
    for (const listener of this.listeners.get(type) ?? []) listener(event)
  }

  /** Deliver one server frame as the socket would. */
  deliver(frame: Record<string, unknown>) {
    this.emit('message', { data: JSON.stringify({ version: 1, ...frame }) })
  }

  frames(): Array<Record<string, any>> {
    return this.sent.map((value) => JSON.parse(value))
  }
}

/** Build a client over a controllable socket and manual clock. */
function harness(options: Record<string, unknown> = {}) {
  const sockets: FakeSocket[] = []
  const timers: Array<{ run: () => void; delay: number }> = []
  let clock = 1000
  const client = createCollabClient({
    room: 'doc:1',
    url: 'http://localhost:3000/__ruvyxa/collab',
    webSocket: (url: string) => {
      const socket = new FakeSocket(url)
      sockets.push(socket)
      return socket as never
    },
    random: () => 0.5,
    now: () => clock,
    setTimeout: (run: () => void, delay: number) => {
      timers.push({ run, delay })
      return timers.length - 1
    },
    clearTimeout: () => {},
    ...options,
  } as never)
  return {
    client,
    sockets,
    timers,
    advance: (ms: number) => {
      clock += ms
    },
    runTimers: () => {
      const pending = timers.splice(0, timers.length)
      for (const timer of pending) timer.run()
    },
  }
}

describe('collab()', () => {
  it('claims the presence capability and rejects short-lived builds', async () => {
    const plugin = collab({ path: '/rooms', heartbeatMs: 10_000 })
    const claims: Array<{ capability: string; options: unknown }> = []
    let buildHook: ((context: any) => void | Promise<void>) | undefined
    await plugin.register({
      environment: 'production',
      http: { onRequest() {}, onResponse() {}, route() {} },
      build: {
        onStart() {},
        onResolve() {},
        onLoad() {},
        onTransform() {},
        onComplete(hook: (context: any) => void | Promise<void>) {
          buildHook = hook
        },
      },
      dev: { onFileChange() {} },
      diagnostics: { report() {} },
      native: {
        claim(capability: string, options: unknown) {
          claims.push({ capability, options })
        },
      },
    } as never)

    assert.deepEqual(claims, [
      { capability: 'presence@1', options: { path: '/rooms', heartbeatMs: 10_000 } },
    ])
    assert.doesNotThrow(() => buildHook?.({ manifest: { target: 'node', adapter: 'node' } }))
    assert.throws(
      () => buildHook?.({ manifest: { target: 'edge', adapter: 'cloudflare' } }),
      /RUV3201.*long-lived/,
    )
    assert.throws(
      () => buildHook?.({ manifest: { target: 'node', adapter: { name: 'vercel' } } }),
      /RUV3201.*long-lived/,
    )
  })

  it('rejects a room id the server would refuse', () => {
    assert.throws(() => createCollabClient({ room: '' } as never), /1-128 letters/)
    assert.throws(() => createCollabClient({ room: 'doc 1' } as never), /1-128 letters/)
    assert.throws(() => createCollabClient({ room: 'a'.repeat(129) } as never), /1-128 letters/)
  })
})

describe('createCollabClient()', () => {
  it('carries the room on the socket URL and upgrades the protocol', () => {
    const { sockets } = harness()
    assert.equal(sockets.length, 1)
    assert.equal(sockets[0].url, 'ws://localhost:3000/__ruvyxa/collab?room=doc%3A1')
  })

  it('adopts the welcome snapshot as the room view', () => {
    const { client, sockets } = harness()
    sockets[0].deliver({
      type: 'welcome',
      room: 'doc:1',
      peer: 'p2',
      peers: { p1: { name: 'Ada' } },
      state: { title: { value: 'Draft', version: 3, peer: 'p1' } },
      roomVersion: 3,
    })

    const snapshot = client.snapshot()
    assert.equal(snapshot.connected, true)
    assert.equal(snapshot.self, 'p2')
    assert.equal(snapshot.roomVersion, 3)
    assert.deepEqual(snapshot.state.title, { value: 'Draft', version: 3, peer: 'p1' })
    assert.deepEqual(
      snapshot.peers.map((peer) => [peer.id, peer.self]),
      [
        ['p1', false],
        ['p2', true],
      ],
    )
  })

  it('tracks peers joining, publishing presence, and leaving', () => {
    const { client, sockets } = harness()
    const seen: number[] = []
    client.subscribe((snapshot) => seen.push(snapshot.peers.length))
    sockets[0].deliver({ type: 'welcome', peer: 'p1', peers: {}, state: {}, roomVersion: 0 })
    sockets[0].deliver({ type: 'join', peer: 'p2' })
    sockets[0].deliver({ type: 'presence', peer: 'p2', state: { cursor: [4, 8] } })

    const peer = client.snapshot().peers.find((entry) => entry.id === 'p2')
    assert.deepEqual(peer?.state, { cursor: [4, 8] })

    sockets[0].deliver({ type: 'leave', peer: 'p2' })
    assert.equal(client.snapshot().peers.length, 1)
    assert.deepEqual(seen, [1, 2, 2, 1])
  })

  it('applies patches and deletes keys written as null', () => {
    const { client, sockets } = harness()
    sockets[0].deliver({ type: 'welcome', peer: 'p1', peers: {}, state: {}, roomVersion: 0 })
    sockets[0].deliver({ type: 'patch', peer: 'p2', roomVersion: 1, entries: { title: 'A' } })
    assert.equal(client.snapshot().state.title.value, 'A')
    assert.equal(client.snapshot().state.title.peer, 'p2')

    sockets[0].deliver({ type: 'patch', peer: 'p3', roomVersion: 2, entries: { title: 'B' } })
    assert.equal(client.snapshot().state.title.value, 'B')
    assert.equal(client.snapshot().roomVersion, 2)

    sockets[0].deliver({ type: 'patch', peer: 'p3', roomVersion: 3, entries: { title: null } })
    assert.equal(client.snapshot().state.title, undefined)
  })

  it('throttles presence into one trailing frame and reflects it locally first', () => {
    const { client, sockets, advance, runTimers } = harness({ presenceThrottleMs: 50 })
    sockets[0].deliver({ type: 'welcome', peer: 'p1', peers: {}, state: {}, roomVersion: 0 })

    client.setPresence({ cursor: 1 })
    client.setPresence({ cursor: 2 })
    client.setPresence({ cursor: 3 })

    // The local peer shows the newest value immediately, without waiting for
    // the socket to accept anything.
    assert.deepEqual(client.snapshot().peers[0].state, { cursor: 3 })
    const presence = sockets[0].frames().filter((frame) => frame.type === 'presence')
    assert.equal(presence.length, 1, 'only the leading frame is sent inside the window')
    assert.deepEqual(presence[0].state, { cursor: 1 })

    advance(50)
    runTimers()
    const flushed = sockets[0].frames().filter((frame) => frame.type === 'presence')
    assert.equal(flushed.length, 2)
    assert.deepEqual(flushed[1].state, { cursor: 3 }, 'the trailing send carries the newest value')
  })

  it('republishes local presence after reconnecting', () => {
    const { client, sockets, advance, runTimers } = harness()
    sockets[0].deliver({ type: 'welcome', peer: 'p1', peers: {}, state: {}, roomVersion: 0 })
    client.setPresence({ name: 'Ada' })
    advance(1000)

    sockets[0].emit('close')
    assert.equal(client.snapshot().connected, false)
    assert.equal(client.snapshot().peers.length, 0, 'a disconnected client shows no peers')

    runTimers()
    assert.equal(sockets.length, 2, 'the close scheduled a reconnect')
    sockets[1].deliver({ type: 'welcome', peer: 'p9', peers: {}, state: {}, roomVersion: 0 })

    const republished = sockets[1].frames().filter((frame) => frame.type === 'presence')
    assert.deepEqual(
      republished.map((frame) => frame.state),
      [{ name: 'Ada' }],
      'the server dropped presence with the old socket, so it is sent again',
    )
    assert.deepEqual(client.snapshot().peers[0].state, { name: 'Ada' })
  })

  it('reconnects for a fresh snapshot when the server reports a lag resync', () => {
    const { sockets } = harness()
    sockets[0].deliver({ type: 'welcome', peer: 'p1', peers: {}, state: {}, roomVersion: 0 })
    sockets[0].deliver({ type: 'resync', reason: 'lagged' })
    assert.equal(sockets[0].closed, true)
    assert.equal(sockets.length, 2, 'a lagged view is replaced, not patched')
  })

  it('reports server rejections without closing the room', () => {
    const { client, sockets } = harness()
    const errors: string[] = []
    client.onError((message) => errors.push(message))
    sockets[0].deliver({ type: 'welcome', peer: 'p1', peers: {}, state: {}, roomVersion: 0 })
    sockets[0].deliver({ type: 'error', message: 'Collaboration room is full' })
    assert.deepEqual(errors, ['Collaboration room is full'])
    assert.equal(client.snapshot().connected, true)
  })

  it('ignores frames from another protocol version or a stale socket', () => {
    const { client, sockets } = harness()
    sockets[0].deliver({ type: 'welcome', peer: 'p1', peers: {}, state: {}, roomVersion: 0 })
    sockets[0].emit('message', { data: JSON.stringify({ version: 2, type: 'patch' }) })
    sockets[0].emit('message', { data: 'not json' })
    sockets[0].emit('message', { data: 42 })
    assert.deepEqual(client.snapshot().state, {})

    sockets[0].deliver({ type: 'resync', reason: 'lagged' })
    // The replaced socket must not be able to write into the room any more.
    sockets[0].deliver({ type: 'patch', peer: 'p2', roomVersion: 9, entries: { title: 'stale' } })
    assert.equal(client.snapshot().state.title, undefined)
  })

  it('sends writes as one batch and rejects a non-object', () => {
    const { client, sockets } = harness()
    sockets[0].deliver({ type: 'welcome', peer: 'p1', peers: {}, state: {}, roomVersion: 0 })
    client.setState({ title: 'A', body: 'B' })
    client.setState({})
    assert.throws(() => client.setState([] as never), /object of keys/)

    const writes = sockets[0].frames().filter((frame) => frame.type === 'set')
    assert.equal(writes.length, 1, 'an empty write is not worth a frame')
    assert.deepEqual(writes[0].entries, { title: 'A', body: 'B' })
  })

  it('stops reconnecting once closed', () => {
    const { client, sockets, timers } = harness()
    sockets[0].deliver({ type: 'welcome', peer: 'p1', peers: {}, state: {}, roomVersion: 0 })
    client.close()
    assert.equal(sockets[0].closed, true)
    assert.equal(client.snapshot().connected, false)

    sockets[0].emit('close')
    assert.equal(timers.length, 0, 'a closed client schedules no reconnect')
    assert.equal(sockets.length, 1)
  })
})
