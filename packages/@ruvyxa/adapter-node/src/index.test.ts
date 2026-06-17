import { describe, expect, it } from "vitest"

import { nodeAdapter } from "./index.js"

describe("nodeAdapter", () => {
  it("returns node deployment output", async () => {
    const adapter = nodeAdapter()
    const output = await adapter.build({ root: ".", outDir: ".ruvyxa" })

    expect(adapter.name).toBe("node")
    expect(adapter.target).toBe("node")
    expect(output).toEqual({
      name: "node",
      target: "node",
      entry: ".ruvyxa/server/app",
      assetsDir: ".ruvyxa/assets",
    })
  })
})
