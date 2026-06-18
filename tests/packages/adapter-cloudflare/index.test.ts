import { describe, it } from "node:test"
import assert from "node:assert/strict"

import { cloudflareAdapter } from "../../../packages/@ruvyxa/adapter-cloudflare/src/index.ts"

describe("cloudflareAdapter", () => {
  it("returns edge deployment output", async () => {
    const output = await cloudflareAdapter().build({ root: ".", outDir: ".ruvyxa" })

    assert.deepEqual({
      name: output.name,
      target: output.target,
      platform: output.platform,
      entry: output.entry,
      assetsDir: output.assetsDir,
    }, {
      name: "cloudflare",
      target: "edge",
      platform: "cloudflare",
      entry: ".ruvyxa/server/app",
      assetsDir: ".ruvyxa/assets",
    })
  })
})
