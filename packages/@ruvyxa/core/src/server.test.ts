import { describe, expect, it, beforeEach } from "vitest"

import { action, cache, cacheStats, invalidateCache, loader, redirect } from "./server.js"

describe("server API", () => {
  beforeEach(() => {
    invalidateCache()
  })

  it("runs loaders with default context", async () => {
    const getValue = loader(async ({ params }) => params.id ?? "missing")
    await expect(getValue()).resolves.toBe("missing")
    await expect(getValue({ params: { id: "123" } })).resolves.toBe("123")
  })

  it("validates action input through schema", async () => {
    const save = action
      .input({ parse: (value: unknown) => String(value).trim() })
      .handler(async ({ input }) => input.toUpperCase())

    await expect(save(" hello ")).resolves.toBe("HELLO")
  })

  it("creates redirect responses", () => {
    const response = redirect("/login")
    expect(response.status).toBe(302)
    expect(response.headers.get("Location")).toBe("/login")
  })

  it("rejects non-3xx redirect status codes", () => {
    expect(() => redirect("/login", 200)).toThrow("redirect() status must be 3xx")
  })
})

describe("cache", () => {
  beforeEach(() => {
    invalidateCache()
  })

  it("caches values and returns them on subsequent calls", async () => {
    let calls = 0
    const producer = () => { calls++; return "value" }

    const first = await cache("test-key").ttl("10s").get(producer)
    const second = await cache("test-key").ttl("10s").get(producer)

    expect(first).toBe("value")
    expect(second).toBe("value")
    expect(calls).toBe(1)
  })

  it("invalidates by exact key", async () => {
    let calls = 0
    const producer = () => { calls++; return `call-${calls}` }

    await cache("k1").ttl("10s").get(producer)
    invalidateCache("k1")
    const result = await cache("k1").ttl("10s").get(producer)

    expect(result).toBe("call-2")
    expect(calls).toBe(2)
  })

  it("invalidates by prefix", async () => {
    await cache("users:list").ttl("10s").get(() => "list")
    await cache("users:detail:1").ttl("10s").get(() => "detail")
    await cache("posts:list").ttl("10s").get(() => "posts")

    invalidateCache("users")

    let userCalls = 0
    let postCalls = 0
    await cache("users:list").ttl("10s").get(() => { userCalls++; return "new-list" })
    await cache("posts:list").ttl("10s").get(() => { postCalls++; return "new-posts" })

    expect(userCalls).toBe(1) // was invalidated, so producer ran
    expect(postCalls).toBe(0) // was NOT invalidated, still cached
  })

  it("reports cache stats", async () => {
    await cache("a").ttl("10s").get(() => 1)
    await cache("b").ttl("10s").get(() => 2)

    const stats = cacheStats()
    expect(stats.size).toBe(2)
    expect(stats.maxEntries).toBe(1024)
  })

  it("returns stale value when producer fails and stale data exists", async () => {
    await cache("fragile").ttl("1ms").get(() => "good")

    // Wait for TTL to expire
    await new Promise((r) => setTimeout(r, 5))

    const result = await cache("fragile").ttl("1ms").get(() => { throw new Error("oops") })
    expect(result).toBe("good")
  })

  it("throws when producer fails and no stale data exists", async () => {
    await expect(
      cache("nonexistent").ttl("10s").get(() => { throw new Error("fail") }),
    ).rejects.toThrow("fail")
  })
})
