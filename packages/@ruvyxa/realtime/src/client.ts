export interface RealtimeActionEvent {
  version: 1
  type: 'action'
  channels: string[]
  action: string
  path: string
  invalidated: string[]
}

export interface RealtimeResyncEvent {
  version: 1
  type: 'resync'
  reason: 'lagged'
}

export type RealtimeEvent = RealtimeActionEvent | RealtimeResyncEvent
export type RealtimeListener = (event: RealtimeEvent) => void

export interface WebSocketLike {
  readonly readyState: number
  addEventListener(
    type: 'open' | 'close' | 'message' | 'error',
    listener: (event: any) => void,
  ): void
  close(code?: number, reason?: string): void
}

export interface RealtimeClientOptions {
  /** Absolute ws(s) URL or application-relative endpoint. @default "/__ruvyxa/realtime" */
  url?: string
  minReconnectMs?: number
  maxReconnectMs?: number
  webSocket?: (url: string) => WebSocketLike
  random?: () => number
}

export interface RealtimeClient {
  subscribe(channel: string, listener: RealtimeListener): () => void
  subscribeRoute(pathname: string, listener: RealtimeListener): () => void
  close(): void
}

/** Create a reconnecting channel client. Subscription changes reconnect with one bounded URL. */
export function createRealtimeClient(options: RealtimeClientOptions = {}): RealtimeClient {
  const minReconnectMs = boundedDelay(options.minReconnectMs ?? 500, 'minReconnectMs')
  const maxReconnectMs = boundedDelay(options.maxReconnectMs ?? 30_000, 'maxReconnectMs')
  if (maxReconnectMs < minReconnectMs) {
    throw new TypeError('Realtime maxReconnectMs must be greater than or equal to minReconnectMs')
  }
  const listeners = new Map<string, Set<RealtimeListener>>()
  const createSocket = options.webSocket ?? ((url: string) => new WebSocket(url))
  const random = options.random ?? Math.random
  let socket: WebSocketLike | undefined
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined
  let generation = 0
  let attempts = 0
  let stopped = false

  const connect = () => {
    if (stopped || listeners.size === 0) return
    const currentGeneration = ++generation
    socket?.close(1000, 'subscriptions changed')
    socket = createSocket(socketUrl(options.url, [...listeners.keys()]))
    socket.addEventListener('open', () => {
      if (currentGeneration === generation) attempts = 0
    })
    socket.addEventListener('message', (message) => {
      if (currentGeneration !== generation || typeof message.data !== 'string') return
      const event = parseEvent(message.data)
      if (!event) return
      if (event.type === 'resync') {
        const notified = new Set<RealtimeListener>()
        for (const group of listeners.values()) {
          for (const listener of group) {
            if (!notified.has(listener)) listener(event)
            notified.add(listener)
          }
        }
        return
      }
      const notified = new Set<RealtimeListener>()
      for (const channel of event.channels) {
        for (const listener of listeners.get(channel) ?? []) {
          if (!notified.has(listener)) listener(event)
          notified.add(listener)
        }
      }
    })
    socket.addEventListener('close', () => {
      if (currentGeneration !== generation || stopped || listeners.size === 0) return
      attempts++
      const exponential = Math.min(maxReconnectMs, minReconnectMs * 2 ** (attempts - 1))
      const delay = Math.round(exponential * (0.75 + random() * 0.5))
      reconnectTimer = setTimeout(connect, delay)
    })
  }

  const refresh = () => {
    clearTimeout(reconnectTimer)
    if (listeners.size === 0) {
      generation++
      socket?.close(1000, 'no subscriptions')
      socket = undefined
      return
    }
    connect()
  }

  return Object.freeze({
    subscribe(channel: string, listener: RealtimeListener) {
      const normalized = validateChannel(channel)
      if (typeof listener !== 'function')
        throw new TypeError('Realtime listener must be a function')
      if (!listeners.has(normalized) && listeners.size >= 16) {
        throw new TypeError('Realtime clients accept at most 16 active channels')
      }
      const group = listeners.get(normalized) ?? new Set<RealtimeListener>()
      const changed = !group.has(listener)
      group.add(listener)
      listeners.set(normalized, group)
      if (changed) refresh()
      let active = true
      return () => {
        if (!active) return
        active = false
        const current = listeners.get(normalized)
        current?.delete(listener)
        if (current?.size === 0) listeners.delete(normalized)
        refresh()
      }
    },
    subscribeRoute(pathname: string, listener: RealtimeListener) {
      if (!pathname.startsWith('/') || pathname.includes('?') || pathname.includes('#')) {
        throw new TypeError('Realtime route subscriptions require an absolute pathname')
      }
      return this.subscribe(routeChannel(pathname), listener)
    },
    close() {
      stopped = true
      generation++
      clearTimeout(reconnectTimer)
      listeners.clear()
      socket?.close(1000, 'client closed')
      socket = undefined
    },
  })
}

function socketUrl(configured: string | undefined, channels: string[]): string {
  const locationValue = globalThis.location
  const base = configured ?? '/__ruvyxa/realtime'
  const url = new URL(base, locationValue?.href ?? 'http://localhost/')
  if (url.protocol === 'http:') url.protocol = 'ws:'
  if (url.protocol === 'https:') url.protocol = 'wss:'
  if (!['ws:', 'wss:'].includes(url.protocol)) {
    throw new TypeError('Realtime URL must use ws:, wss:, http:, https:, or an application path')
  }
  url.searchParams.set('channels', channels.join(','))
  return url.href
}

function parseEvent(value: string): RealtimeEvent | null {
  try {
    const event = JSON.parse(value) as Partial<RealtimeEvent>
    if (event.version !== 1) return null
    if (event.type === 'resync' && event.reason === 'lagged') return event as RealtimeResyncEvent
    if (
      event.type === 'action' &&
      Array.isArray(event.channels) &&
      event.channels.length > 0 &&
      event.channels.length <= 16 &&
      event.channels.every(
        (channel) => typeof channel === 'string' && /^[A-Za-z0-9:._/-]{1,128}$/.test(channel),
      ) &&
      typeof event.action === 'string' &&
      event.action.length <= 256 &&
      typeof event.path === 'string' &&
      event.path.startsWith('/') &&
      event.path.length <= 2048 &&
      Array.isArray(event.invalidated) &&
      event.invalidated.every((key) => typeof key === 'string' && key.length <= 256)
    ) {
      return event as RealtimeActionEvent
    }
    return null
  } catch {
    return null
  }
}

function validateChannel(value: string): string {
  const channel = value.trim()
  if (!/^[A-Za-z0-9:._/-]{1,128}$/.test(channel)) {
    throw new TypeError(
      'Realtime channels use 1-128 letters, digits, colon, dot, underscore, slash, or dash',
    )
  }
  return channel
}

function routeChannel(pathname: string): string {
  const readable = `route:${pathname}`
  if (readable.length <= 128) return readable
  let hash = 0xcbf29ce484222325n
  for (const character of pathname) {
    hash ^= BigInt(character.codePointAt(0)!)
    hash = BigInt.asUintN(64, hash * 0x100000001b3n)
  }
  return `route-hash:${hash.toString(16).padStart(16, '0')}`
}

function boundedDelay(value: number, name: string): number {
  if (!Number.isSafeInteger(value) || value < 100 || value > 300_000) {
    throw new TypeError(`Realtime ${name} must be between 100 and 300000`)
  }
  return value
}
