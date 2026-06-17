import { describe, expect, it } from "vitest"

import { bunAdapter } from "./index.js"

describe("bunAdapter", () => {
  it("returns bun deployment output", async () => {
    const output = await bunAdapter().build({ root: ".", outDir: ".ruvyxa" })

    expect(output).toMatchObject({
      name: "bun",
      target: "node",
      platform: "bun",
      entry: ".ruvyxa/server/app",
      assetsDir: ".ruvyxa/assets",
    })
  })
})
