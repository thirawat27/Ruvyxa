import { describe, expect, it } from "vitest"

import { cloudflareAdapter } from "./index.js"

describe("cloudflareAdapter", () => {
  it("returns edge deployment output", async () => {
    const output = await cloudflareAdapter().build({ root: ".", outDir: ".ruvyxa" })

    expect(output).toMatchObject({
      name: "cloudflare",
      target: "edge",
      platform: "cloudflare",
      entry: ".ruvyxa/server/app",
      assetsDir: ".ruvyxa/assets",
    })
  })
})
