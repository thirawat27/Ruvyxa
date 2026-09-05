import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { realtime } from '../../../packages/@ruvyxa/realtime/dist/index.js'
import { createRealtimeClient } from '../../../packages/@ruvyxa/realtime/dist/client.js'
import { realtime as realtimeEntry } from '../../../packages/@ruvyxa/realtime/dist/plugin.js'

class FakeSocket {
  readyState = 0
  listeners = new Map<string, Array<(event: any) => void>>()
  closed = false
  readonly url: string

  constructor(url: string) {
    this.url = url
  }

  addEventListener(type: string, listener: (event: any) => void) {
    const values = this.listeners.get(type) ?? []
    values.push(listener)
    this.listeners.set(type, values)
  }

  close() {
    this.closed = true
  }

  emit(type: string, event: any = {}) {
    for (const listener of this.listeners.get(type) ?? []) listener(event)
  }
}

describe('@ruvyxa/realtime', () => {
  it('claims the native transport and registers no deployment gate of its own', async () => {
    assert.equal(realtimeEntry, realtime)
    const plugin = realtimeEntry({ path: '/events', heartbeatMs: 10_000, capacity: 64 })
    const claims: Array<{ capability: string; options: unknown }> = []
    const buildHooks: unknown[] = []
    await plugin.register({
      environment: 'production',
      http: { onRequest() {}, onResponse() {}, route() {} },
      build: {
        onStart() {},
        onResolve() {},
        onLoad() {},
        onTransform() {},
        onComplete(value) {
          buildHooks.push(value)
        },
      },
      dev: { onFileChange() {} },
      diagnostics: { report() {} },
      native: {
        claim(capability, value) {
          claims.push({ capability, options: value })
        },
      },
    })
    assert.deepEqual(claims, [
      { capability: 'realtime@1', options: { path: '/events', heartbeatMs: 10_000, capacity: 64 } },
    ])
    // Whether a target can serve the socket is decided where the socket is
    // served — `adapter-runner.mjs` reports RUV2205 for every build artifact,
    // because none of them holds a connection. A gate here was a second owner
    // of that rule with the opposite premise: it refused vercel outright and
    // let a railway build ship `/__ruvyxa/realtime` as a 404.
    assert.deepEqual(buildHooks, [])
  })

  it('routes action events only to matching channel listeners', () => {
    const sockets: FakeSocket[] = []
    const received: string[] = []
    const client = createRealtimeClient({
      url: 'wss://app.example.com/events',
      // Reconnects are coalesced into a deferred task; run it inline so the
      // assertions below observe the socket without awaiting a microtask.
      scheduleRefresh: (run) => run(),
      webSocket(url) {
        const socket = new FakeSocket(url)
        sockets.push(socket)
        return socket
      },
    })
    const unsubscribe = client.subscribe('todos', (event) => received.push(event.type))
    assert.equal(new URL(sockets[0]!.url).searchParams.get('channels'), 'todos')
    sockets[0]!.emit('message', {
      data: JSON.stringify({
        version: 1,
        type: 'action',
        channels: ['users'],
        action: 'save',
        path: '/',
        invalidated: [],
      }),
    })
    sockets[0]!.emit('message', {
      data: JSON.stringify({
        version: 1,
        type: 'action',
        channels: ['todos'],
        action: 'save',
        path: '/todos',
        invalidated: ['todos'],
      }),
    })
    assert.deepEqual(received, ['action'])
    unsubscribe()
    assert.equal(sockets[0]!.closed, true)
  })

  it('opens one socket for a burst of subscriptions instead of one per channel', async () => {
    // The socket URL carries the whole channel set, so each change needs a new
    // connection. Acting per change opened and discarded N-1 sockets while a
    // component tree mounted — N handshakes to reach one useful connection.
    const sockets: FakeSocket[] = []
    const client = createRealtimeClient({
      url: 'ws://localhost/events',
      webSocket(url) {
        const socket = new FakeSocket(url)
        sockets.push(socket)
        return socket
      },
    })

    const listener = () => {}
    const stops = [
      client.subscribe('todos', listener),
      client.subscribe('users', listener),
      client.subscribeRoute('/todos', listener),
    ]
    assert.equal(sockets.length, 0, 'no socket opens before the burst settles')

    await Promise.resolve()

    assert.equal(sockets.length, 1, 'the whole burst produces one connection')
    assert.equal(
      new URL(sockets[0]!.url).searchParams.get('channels'),
      'todos,users,route:/todos',
      'the single connection carries the final channel set',
    )

    // Unsubscribing in a burst must collapse the same way.
    for (const stop of stops) stop()
    await Promise.resolve()
    assert.equal(sockets.length, 1, 'tearing down opens no further sockets')
    assert.equal(sockets[0]!.closed, true)
    client.close()
  })

  it('deduplicates resync notifications and validates route channels', () => {
    const sockets: FakeSocket[] = []
    let resyncs = 0
    const listener = () => resyncs++
    const client = createRealtimeClient({
      url: 'ws://localhost/events',
      scheduleRefresh: (run) => run(),
      webSocket(url) {
        const socket = new FakeSocket(url)
        sockets.push(socket)
        return socket
      },
    })
    client.subscribe('todos', listener)
    client.subscribeRoute('/todos', listener)
    const latest = sockets.at(-1)!
    latest.emit('message', {
      data: JSON.stringify({ version: 1, type: 'resync', reason: 'lagged' }),
    })
    assert.equal(resyncs, 1)
    const longPath = `/${'segment/'.repeat(30)}`
    client.subscribeRoute(longPath, listener)
    assert.equal(
      new URL(sockets.at(-1)!.url).searchParams.get('channels'),
      'todos,route:/todos,route-hash:64d412af0acae2fa',
    )
    assert.throws(() => client.subscribe('bad,channel', listener), /Realtime channels/)
    for (let index = 0; index < 13; index++) client.subscribe(`channel-${index}`, listener)
    assert.throws(() => client.subscribe('channel-overflow', listener), /at most 16/)
    client.close()
  })
})
