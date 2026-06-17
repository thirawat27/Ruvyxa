import { describe, expect, it } from "vitest"

import { netlifyAdapter } from "./index.js"

describe("netlifyAdapter", () => {
  it("returns serverless deployment output", async () => {
    const output = await netlifyAdapter().build({ root: ".", outDir: ".ruvyxa" })

    expect(output).toMatchObject({
      name: "netlify",
      target: "serverless",
      platform: "netlify",
      entry: ".ruvyxa/server/app",
      assetsDir: ".ruvyxa/assets",
      functionsDir: ".ruvyxa/netlify/functions",
    })
  })
})
