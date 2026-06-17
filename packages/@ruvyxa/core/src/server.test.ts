import { describe, expect, it } from "vitest"

import { action, loader, redirect } from "./server.js"

describe("server API", () => {
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
})
