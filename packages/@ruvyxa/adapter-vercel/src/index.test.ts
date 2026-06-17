import { describe, expect, it } from "vitest"

import { vercelAdapter } from "./index.js"

describe("vercelAdapter", () => {
  it("returns serverless deployment output", async () => {
    const output = await vercelAdapter().build({ root: ".", outDir: ".ruvyxa" })

    expect(output).toMatchObject({
      name: "vercel",
      target: "serverless",
      platform: "vercel",
      entry: ".ruvyxa/server/app",
      assetsDir: ".ruvyxa/assets",
      functionsDir: ".ruvyxa/functions",
    })
  })
})
