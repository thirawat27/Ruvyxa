import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { realtime } from '../../../packages/@ruvyxa/realtime/dist/index.js'
import { createRealtimeClient } from '../../../packages/@ruvyxa/realtime/dist/client.js'

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
  it('registers one validated native transport and rejects unsupported builds', async () => {
    const plugin = realtime({ path: '/events', heartbeatMs: 10_000, capacity: 64 })
    let registered: unknown
    let buildHook: ((context: any) => void | Promise<void>) | undefined
    await plugin.setup({
      addMiddleware() {},
      resolveId() {},
      transform() {},
      enableRealtime(value) {
        registered = value
      },
      onBuildComplete(value) {
        buildHook = value
      },
    })
    assert.deepEqual(registered, { path: '/events', heartbeatMs: 10_000, capacity: 64 })
    await buildHook?.({ manifest: { target: 'node', adapter: 'node' } })
    await buildHook?.({ manifest: { target: 'node', adapter: { name: 'bun' } } })
    await assert.rejects(
      async () => buildHook?.({ manifest: { target: 'edge', adapter: 'cloudflare' } }),
      /RUV3201.*self-hosted/,
    )
    await assert.rejects(
      async () => buildHook?.({ manifest: { target: 'node', adapter: 'vercel' } }),
      /RUV3201.*self-hosted/,
    )
  })

  it('routes action events only to matching channel listeners', () => {
    const sockets: FakeSocket[] = []
    const received: string[] = []
    const client = createRealtimeClient({
      url: 'wss://app.example.com/events',
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

  it('deduplicates resync notifications and validates route channels', () => {
    const sockets: FakeSocket[] = []
    let resyncs = 0
    const listener = () => resyncs++
    const client = createRealtimeClient({
      url: 'ws://localhost/events',
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
