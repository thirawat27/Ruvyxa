/**
 * Client for Ruvyxa's native collaboration rooms.
 *
 * Two kinds of state travel over one socket, and they behave differently on
 * purpose:
 *
 * - **Presence** is ephemeral. Cursors, selections, and "who is here" are
 *   worthless once stale, so they are dropped the moment a peer disconnects and
 *   are never replayed to a late joiner.
 * - **Shared state** is retained for the life of the room and is
 *   last-writer-wins per key. The server is the only sequencer, so "last" means
 *   "last frame to reach the server" — no client clock is involved, and two
 *   peers writing the same key concurrently converge on the same winner rather
 *   than merging.
 *
 * Shared state is not a CRDT. Concurrent edits to one key do not merge; the
 * later write replaces the earlier one. Model a document as many small keys
 * when you want two peers to edit it at once without overwriting each other.
 */

import type { WebSocketLike } from './client.js'

/** A peer in the room, including the local one. */
export interface CollabPeer {
  readonly id: string
  /** Whatever this peer last published. `null` until it publishes anything. */
  readonly state: unknown
  /** True for the connection reading this snapshot. */
  readonly self: boolean
}

/** One shared-state key and the room version that last wrote it. */
export interface CollabEntry {
  readonly value: unknown
  readonly version: number
  readonly peer: string
}

/** The room as this client currently sees it. */
export interface CollabSnapshot {
  readonly room: string
  /** This connection's peer id, or `undefined` before the room is joined. */
  readonly self: string | undefined
  readonly connected: boolean
  readonly peers: readonly CollabPeer[]
  readonly state: Readonly<Record<string, CollabEntry>>
  /** Server-assigned write counter. Increases by one per accepted write batch. */
  readonly roomVersion: number
}

export type CollabListener = (snapshot: CollabSnapshot) => void
export type CollabErrorListener = (message: string) => void

export interface CollabClientOptions {
  /** Room id. 1-128 letters, digits, colon, dot, underscore, slash, or dash. */
  room: string
  /** Absolute ws(s) URL or application-relative endpoint. @default "/__ruvyxa/collab" */
  url?: string
  minReconnectMs?: number
  maxReconnectMs?: number
  /**
   * Minimum gap between outgoing presence frames. Cursor streams fire far
   * faster than they are worth sending, and the server rejects a connection
   * that exceeds its frame budget, so updates inside the window are collapsed
   * into one trailing send.
   *
   * @default 50
   */
  presenceThrottleMs?: number
  webSocket?: (url: string) => WebSocketLike
  random?: () => number
  /** Injected so tests drive throttling and reconnection without real timers. */
  setTimeout?: (run: () => void, delay: number) => unknown
  clearTimeout?: (handle: unknown) => void
  now?: () => number
}

export interface CollabClient {
  /** Current room view. Safe to read at any time, including before connecting. */
  snapshot(): CollabSnapshot
  /** Observe every room change. Returns an unsubscribe function. */
  subscribe(listener: CollabListener): () => void
  /** Observe server-rejected frames (bad room, rate limit, room full). */
  onError(listener: CollabErrorListener): () => void
  /** Publish this connection's presence, replacing whatever it published before. */
  setPresence(state: unknown): void
  /** Write shared-state keys. A `null` value deletes the key. */
  setState(entries: Record<string, unknown>): void
  close(): void
}

interface ServerFrame {
  version?: number
  type?: string
  room?: string
  peer?: string
  peers?: Record<string, unknown>
  state?: Record<string, { value?: unknown; version?: number; peer?: string }>
  entries?: Record<string, unknown>
  roomVersion?: number
  message?: string
  reason?: string
}

const DEFAULT_PATH = '/__ruvyxa/collab'
const ROOM_PATTERN = /^[A-Za-z0-9:._/-]{1,128}$/

/**
 * Open a collaboration room and keep a local mirror of it.
 *
 * The client reconnects with backoff and re-reads the room from the welcome
 * snapshot each time, so a dropped connection converges rather than replaying
 * a partial history. Local presence is re-published after every reconnect
 * because the server drops presence with the connection that produced it.
 */
export function createCollabClient(options: CollabClientOptions): CollabClient {
  const room = options.room
  if (typeof room !== 'string' || !ROOM_PATTERN.test(room)) {
    throw new TypeError(
      'Collaboration rooms use 1-128 letters, digits, colon, dot, underscore, slash, or dash',
    )
  }
  const minReconnectMs = boundedDelay(options.minReconnectMs ?? 500, 'minReconnectMs')
  const maxReconnectMs = boundedDelay(options.maxReconnectMs ?? 30_000, 'maxReconnectMs')
  if (maxReconnectMs < minReconnectMs) {
    throw new TypeError('Collaboration maxReconnectMs must be at least minReconnectMs')
  }
  const presenceThrottleMs = options.presenceThrottleMs ?? 50
  if (
    !Number.isInteger(presenceThrottleMs) ||
    presenceThrottleMs < 0 ||
    presenceThrottleMs > 5000
  ) {
    throw new TypeError('Collaboration presenceThrottleMs must be between 0 and 5000')
  }

  const createSocket = options.webSocket ?? ((url: string) => new WebSocket(url))
  const random = options.random ?? Math.random
  const schedule = options.setTimeout ?? ((run, delay) => setTimeout(run, delay))
  const cancel = options.clearTimeout ?? ((handle) => clearTimeout(handle as never))
  const now = options.now ?? (() => Date.now())

  const listeners = new Set<CollabListener>()
  const errorListeners = new Set<CollabErrorListener>()
  let socket: WebSocketLike | undefined
  let reconnectTimer: unknown
  let presenceTimer: unknown
  let generation = 0
  let attempts = 0
  let stopped = false
  let lastPresenceSentAt = Number.NEGATIVE_INFINITY
  let pendingPresence: { state: unknown } | undefined
  // Retained across reconnects: the server discards presence with the socket,
  // so the local peer must republish itself to reappear for everyone else.
  let localPresence: unknown = null

  let self: string | undefined
  let connected = false
  let peers = new Map<string, unknown>()
  let state = new Map<string, CollabEntry>()
  let roomVersion = 0
  let view: CollabSnapshot = buildSnapshot()

  function buildSnapshot(): CollabSnapshot {
    return Object.freeze({
      room,
      self,
      connected,
      peers: Object.freeze(
        [...peers.entries()].map(([id, peerState]) =>
          Object.freeze({ id, state: peerState, self: id === self }),
        ),
      ),
      state: Object.freeze(Object.fromEntries(state)),
      roomVersion,
    })
  }

  function publish(): void {
    view = buildSnapshot()
    for (const listener of listeners) listener(view)
  }

  function fail(message: string): void {
    for (const listener of errorListeners) listener(message)
  }

  function send(frame: Record<string, unknown>): boolean {
    if (!socket || socket.readyState !== 1) {
      return false
    }
    const open = socket as unknown as { send(data: string): void }
    open.send(JSON.stringify({ version: 1, ...frame }))
    return true
  }

  function applyFrame(frame: ServerFrame): void {
    switch (frame.type) {
      case 'welcome': {
        self = typeof frame.peer === 'string' ? frame.peer : undefined
        connected = true
        peers = new Map(Object.entries(frame.peers ?? {}))
        if (self !== undefined) peers.set(self, localPresence)
        state = new Map(
          Object.entries(frame.state ?? {}).map(([key, entry]) => [
            key,
            Object.freeze({
              value: entry?.value ?? null,
              version: typeof entry?.version === 'number' ? entry.version : 0,
              peer: typeof entry?.peer === 'string' ? entry.peer : '',
            }),
          ]),
        )
        roomVersion = typeof frame.roomVersion === 'number' ? frame.roomVersion : 0
        // Republish immediately so peers already in the room see this one with
        // its real state rather than the null it was announced with.
        if (localPresence !== null) send({ type: 'presence', state: localPresence })
        publish()
        return
      }
      case 'join': {
        if (typeof frame.peer !== 'string') return
        peers.set(frame.peer, null)
        publish()
        return
      }
      case 'presence': {
        if (typeof frame.peer !== 'string') return
        peers.set(frame.peer, frame.state ?? null)
        publish()
        return
      }
      case 'leave': {
        if (typeof frame.peer !== 'string' || !peers.delete(frame.peer)) return
        publish()
        return
      }
      case 'patch': {
        const version = typeof frame.roomVersion === 'number' ? frame.roomVersion : roomVersion
        const author = typeof frame.peer === 'string' ? frame.peer : ''
        for (const [key, value] of Object.entries(frame.entries ?? {})) {
          if (value === null) state.delete(key)
          else state.set(key, Object.freeze({ value, version, peer: author }))
        }
        roomVersion = version
        publish()
        return
      }
      case 'resync': {
        // The server dropped this peer's place in the room's frame buffer, so
        // the local mirror can no longer be trusted. Reconnecting re-reads the
        // whole room from a fresh welcome snapshot.
        reconnect()
        return
      }
      case 'error': {
        fail(typeof frame.message === 'string' ? frame.message : 'Collaboration frame rejected')
        return
      }
      default:
    }
  }

  function connect(): void {
    if (stopped) return
    // A server render has no `location` to dial from, and Node has had a global
    // `WebSocket` since 24 — so a `'use client'` provider rendered for the
    // initial document opened a real connection per request, to
    // `http://localhost/`, and left it open. Staying inert here is what makes
    // the same component safe on both sides: the browser connects on mount.
    if (!options.webSocket && !hasBrowserLocation()) return
    const current = ++generation
    socket = createSocket(collabUrl(options.url, room))
    socket.addEventListener('open', () => {
      if (current === generation) attempts = 0
    })
    socket.addEventListener('message', (message: { data?: unknown }) => {
      if (current !== generation || typeof message.data !== 'string') return
      let frame: ServerFrame
      try {
        frame = JSON.parse(message.data) as ServerFrame
      } catch {
        return
      }
      if (frame.version !== 1) return
      applyFrame(frame)
    })
    socket.addEventListener('close', () => {
      if (current !== generation || stopped) return
      connected = false
      self = undefined
      peers = new Map()
      publish()
      attempts++
      const exponential = Math.min(maxReconnectMs, minReconnectMs * 2 ** (attempts - 1))
      reconnectTimer = schedule(connect, Math.round(exponential * (0.75 + random() * 0.5)))
    })
  }

  function reconnect(): void {
    if (stopped) return
    generation++
    socket?.close(1000, 'resync')
    socket = undefined
    connected = false
    connect()
  }

  function flushPresence(): void {
    presenceTimer = undefined
    if (!pendingPresence || stopped) return
    const { state: next } = pendingPresence
    pendingPresence = undefined
    lastPresenceSentAt = now()
    send({ type: 'presence', state: next })
  }

  connect()

  return Object.freeze({
    snapshot: () => view,
    subscribe(listener: CollabListener) {
      if (typeof listener !== 'function') {
        throw new TypeError('Collaboration listener must be a function')
      }
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    onError(listener: CollabErrorListener) {
      if (typeof listener !== 'function') {
        throw new TypeError('Collaboration error listener must be a function')
      }
      errorListeners.add(listener)
      return () => {
        errorListeners.delete(listener)
      }
    },
    setPresence(next: unknown) {
      localPresence = next
      // Reflect locally before the round trip so a cursor tracks the pointer
      // instead of lagging one network hop behind it.
      if (self !== undefined) {
        peers.set(self, next)
        publish()
      }
      const elapsed = now() - lastPresenceSentAt
      if (elapsed >= presenceThrottleMs) {
        pendingPresence = undefined
        if (presenceTimer !== undefined) {
          cancel(presenceTimer)
          presenceTimer = undefined
        }
        lastPresenceSentAt = now()
        send({ type: 'presence', state: next })
        return
      }
      pendingPresence = { state: next }
      if (presenceTimer === undefined) {
        presenceTimer = schedule(flushPresence, presenceThrottleMs - elapsed)
      }
    },
    setState(entries: Record<string, unknown>) {
      if (!entries || typeof entries !== 'object' || Array.isArray(entries)) {
        throw new TypeError('Collaboration setState expects an object of keys to write')
      }
      if (Object.keys(entries).length === 0) return
      send({ type: 'set', entries })
    },
    close() {
      stopped = true
      generation++
      cancel(reconnectTimer)
      if (presenceTimer !== undefined) cancel(presenceTimer)
      listeners.clear()
      errorListeners.clear()
      socket?.close(1000, 'client closed')
      socket = undefined
      connected = false
      publish()
    },
  })
}

/**
 * Whether this runtime is a browser with an address to dial from.
 *
 * A `'use client'` component still renders on the server for the initial
 * document, and Node has carried a global `WebSocket` since 24 — so nothing
 * stopped a server render from opening a connection, and `collabUrl` fell back
 * to `http://localhost/` when there was no `location` to resolve against. The
 * connection was per render and nothing closed it.
 */
function hasBrowserLocation(): boolean {
  return typeof globalThis.location?.href === 'string'
}

function collabUrl(configured: string | undefined, room: string): string {
  const url = new URL(configured ?? DEFAULT_PATH, globalThis.location?.href ?? 'http://localhost/')
  if (url.protocol === 'http:') url.protocol = 'ws:'
  if (url.protocol === 'https:') url.protocol = 'wss:'
  if (!['ws:', 'wss:'].includes(url.protocol)) {
    throw new TypeError(
      'Collaboration URL must use ws:, wss:, http:, https:, or an application path',
    )
  }
  url.searchParams.set('room', room)
  return url.href
}

function boundedDelay(value: number, name: string): number {
  if (!Number.isSafeInteger(value) || value < 100 || value > 300_000) {
    throw new TypeError(`Collaboration ${name} must be between 100 and 300000`)
  }
  return value
}
