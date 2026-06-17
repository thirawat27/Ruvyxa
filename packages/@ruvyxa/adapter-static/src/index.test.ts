import { describe, expect, it } from "vitest"

import { staticAdapter } from "./index.js"

describe("staticAdapter", () => {
  it("returns static deployment output", async () => {
    const output = await staticAdapter().build({ root: ".", outDir: ".ruvyxa" })

    expect(output).toMatchObject({
      name: "static",
      target: "static",
      platform: "static",
      entry: ".ruvyxa/static",
      assetsDir: ".ruvyxa/assets",
    })
  })
})
