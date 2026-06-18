import { describe, it } from "node:test"
import assert from "node:assert/strict"

import { bunAdapter } from "../../../packages/@ruvyxa/adapter-bun/src/index.ts"

describe("bunAdapter", () => {
  it("returns bun deployment output", async () => {
    const output = await bunAdapter().build({ root: ".", outDir: ".ruvyxa" })

    assert.deepEqual({
      name: output.name,
      target: output.target,
      platform: output.platform,
      entry: output.entry,
      assetsDir: output.assetsDir,
    }, {
      name: "bun",
      target: "node",
      platform: "bun",
      entry: ".ruvyxa/server/app",
      assetsDir: ".ruvyxa/assets",
    })
  })
})
