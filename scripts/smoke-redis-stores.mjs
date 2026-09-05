#!/usr/bin/env node
/**
 * Run `@ruvyxa/auth`'s Redis stores against a real Redis.
 *
 * Usage: node scripts/smoke-redis-stores.mjs
 *   REDIS_HOST (default 127.0.0.1) and REDIS_PORT (default 6379) name the server.
 *
 * The unit suite pins the two Lua scripts as text and models their effect in a
 * fake, which is the most a suite with no Redis in it can do. What it cannot do
 * is prove the scripts run on a server, or that `take` is atomic when eight
 * connections race for one token — the property the whole design exists for. A
 * `GET` + `DEL` pair hands the same token to several of them; the script hands
 * it to exactly one, and that is what this asks.
 *
 * It speaks RESP2 over `node:net` rather than pulling a client in: the package
 * pins no Redis client on purpose, and the smoke should not either. The
 * `RedisCommandPort` it builds is the same shape `nodeRedisCommandPort` and
 * `ioredisCommandPort` produce, so the stores under test are the shipped ones.
 */
import net from 'node:net'
import { pathToFileURL } from 'node:url'
import path from 'node:path'

const host = process.env.REDIS_HOST ?? '127.0.0.1'
const port = Number(process.env.REDIS_PORT ?? 6379)
const repoRoot = path.resolve(import.meta.dirname, '..')
const { redisAuthStore, redisRateLimitStore } = await import(
  pathToFileURL(path.join(repoRoot, 'packages/@ruvyxa/auth/dist/index.js')).href
)

/** One RESP2 connection: commands go out as arrays, replies come back parsed. */
function connect() {
  const socket = net.connect(port, host)
  const queue = []
  let buffer = Buffer.alloc(0)

  socket.on('data', (chunk) => {
    buffer = Buffer.concat([buffer, chunk])
    for (;;) {
      const parsed = parseReply(buffer, 0)
      if (!parsed) return
      buffer = buffer.subarray(parsed.end)
      const pending = queue.shift()
      if (!pending) return
      if (parsed.value instanceof RedisError) pending.reject(parsed.value)
      else pending.resolve(parsed.value)
    }
  })
  socket.on('error', (error) => {
    for (const pending of queue.splice(0)) pending.reject(error)
  })

  const ready = new Promise((resolve, reject) => {
    socket.once('connect', resolve)
    socket.once('error', reject)
  })

  return {
    ready,
    async command(...parts) {
      await ready
      const encoded =
        `*${parts.length}\r\n` +
        parts
          .map((part) => {
            const text = String(part)
            return `$${Buffer.byteLength(text)}\r\n${text}\r\n`
          })
          .join('')
      return new Promise((resolve, reject) => {
        queue.push({ resolve, reject })
        socket.write(encoded)
      })
    },
    close() {
      socket.destroy()
    },
  }
}

class RedisError extends Error {}

/** Parse one RESP2 reply at `offset`, or return null when more bytes are needed. */
function parseReply(buffer, offset) {
  if (offset >= buffer.length) return null
  const lineEnd = buffer.indexOf('\r\n', offset)
  if (lineEnd === -1) return null
  const type = String.fromCharCode(buffer[offset])
  const line = buffer.toString('utf8', offset + 1, lineEnd)
  const next = lineEnd + 2
  switch (type) {
    case '+':
      return { value: line, end: next }
    case '-':
      return { value: new RedisError(line), end: next }
    case ':':
      return { value: Number(line), end: next }
    case '$': {
      const length = Number(line)
      if (length === -1) return { value: null, end: next }
      if (buffer.length < next + length + 2) return null
      return { value: buffer.toString('utf8', next, next + length), end: next + length + 2 }
    }
    case '*': {
      const count = Number(line)
      if (count === -1) return { value: null, end: next }
      const items = []
      let cursor = next
      for (let index = 0; index < count; index += 1) {
        const item = parseReply(buffer, cursor)
        if (!item) return null
        items.push(item.value)
        cursor = item.end
      }
      return { value: items, end: cursor }
    }
    default:
      throw new Error(`unexpected RESP type ${JSON.stringify(type)}`)
  }
}

/** The four-command port over one connection. */
function portOver(connection) {
  return {
    get: (key) => connection.command('GET', key),
    set: (key, value, ttlSeconds) => connection.command('SET', key, value, 'EX', ttlSeconds),
    del: (key) => connection.command('DEL', key),
    eval: (script, keys, args) => connection.command('EVAL', script, keys.length, ...keys, ...args),
  }
}

const prefix = `ruvyxa:smoke:${process.pid}:${Date.now()}:`
const failures = []
function check(name, condition, detail = '') {
  const status = condition ? 'ok' : 'FAIL'
  console.log(`[${status}] ${name}${detail ? ` — ${detail}` : ''}`)
  if (!condition) failures.push(name)
}

const primary = connect()
try {
  await primary.ready
  check('server answers PING', (await primary.command('PING')) === 'PONG')

  const store = redisAuthStore(portOver(primary), { prefix })
  const limiter = redisRateLimitStore(portOver(primary), { prefix })

  // Session round trip with a real EX.
  await store.set('session:one', '{"user":1}', 30)
  check('SET EX then GET returns the value', (await store.get('session:one')) === '{"user":1}')
  const ttl = await primary.command('TTL', `${prefix}session:one`)
  check('the key carries the TTL it was given', ttl > 0 && ttl <= 30, `TTL=${ttl}`)
  await store.delete('session:one')
  check('DELETE removes the key', (await store.get('session:one')) === null)

  // The property the scripts exist for: eight connections race one token.
  await store.set('magic:token', 'ada@example.com', 60)
  const racers = Array.from({ length: 8 }, () => connect())
  await Promise.all(racers.map((racer) => racer.ready))
  const winners = await Promise.all(
    racers.map((racer) => redisAuthStore(portOver(racer), { prefix }).take('magic:token')),
  )
  for (const racer of racers) racer.close()
  const taken = winners.filter((value) => value !== null)
  check(
    'exactly one of eight concurrent take() calls receives the token',
    taken.length === 1 && taken[0] === 'ada@example.com',
    `winners=${JSON.stringify(winners)}`,
  )
  check('take() of a consumed token is null', (await store.take('magic:token')) === null)

  // Fixed window: allowed up to the limit, then denied with a retry inside the window.
  const decisions = []
  for (let index = 0; index < 4; index += 1)
    decisions.push(await limiter.consume('rate:client', 3, 30))
  check(
    'consume() admits exactly `limit` requests',
    decisions.slice(0, 3).every((d) => d.allowed) && !decisions[3].allowed,
    JSON.stringify(decisions.map((d) => [d.allowed, d.remaining, d.retryAfterSeconds])),
  )
  check(
    'a denied request carries a retry inside the window',
    decisions[3].retryAfterSeconds >= 1 && decisions[3].retryAfterSeconds <= 30,
  )

  // A counter that lost its expiry is repaired instead of counting forever.
  await primary.command('PERSIST', `${prefix}rate:client`)
  check(
    'PERSIST removed the TTL (precondition)',
    (await primary.command('TTL', `${prefix}rate:client`)) === -1,
  )
  const repaired = await limiter.consume('rate:client', 3, 30)
  const repairedTtl = await primary.command('TTL', `${prefix}rate:client`)
  check(
    'consume() restores the window on a counter with no TTL',
    repairedTtl > 0 && repairedTtl <= 30 && repaired.retryAfterSeconds === 30,
    `TTL=${repairedTtl} retryAfter=${repaired.retryAfterSeconds}`,
  )

  // Leave nothing behind.
  const keys = await primary.command('KEYS', `${prefix}*`)
  if (keys.length > 0) await primary.command('DEL', ...keys)
} catch (error) {
  failures.push('smoke aborted')
  console.error(`[FAIL] ${error.message}`)
} finally {
  primary.close()
}

if (failures.length > 0) {
  console.error(`redis stores smoke failed: ${failures.join('; ')}`)
  process.exit(1)
}
console.log('redis stores smoke passed')
