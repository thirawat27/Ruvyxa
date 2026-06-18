import { describe, it } from "node:test"
import assert from "node:assert/strict"

import { createRuvyxaApp } from "../../../packages/create-ruvyxa/dist/index.js"

describe("createRuvyxaApp", () => {
  it("rejects Windows reserved project names", async () => {
    await assert.rejects(createRuvyxaApp("CON"), /reserved or unsafe/)
    await assert.rejects(createRuvyxaApp("lpt1.txt"), /reserved or unsafe/)
  })

  it("rejects project names ending with unsafe Windows characters", async () => {
    await assert.rejects(createRuvyxaApp("my-app."), /reserved or unsafe/)
    await assert.rejects(createRuvyxaApp("my-app "), /whitespace/)
  })
})
