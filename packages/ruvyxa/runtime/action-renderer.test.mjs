import { execFile } from "node:child_process"
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { promisify } from "node:util"

import { describe, expect, it } from "vitest"

const execFileAsync = promisify(execFile)
const workspaceRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)))
const exampleRoot = path.join(workspaceRoot, "examples/basic-app")
const renderer = path.join(workspaceRoot, "packages/ruvyxa/runtime/action-renderer.mjs")

describe("action renderer", () => {
  it("invokes exported server actions with JSON input", async () => {
    await withFixture(async ({ actionFile }) => {
      const result = await runRenderer(actionFile, "createTodo", JSON.stringify({ title: "Test" }))

      expect(result.ok).toBe(true)
      expect(result.status).toBe(200)
      expect(JSON.parse(result.body)).toEqual({
        data: { title: "Test", completed: false },
        invalidated: ["todos"],
      })
    })
  })

  it("invokes exported server actions with form input", async () => {
    await withFixture(async ({ actionFile }) => {
      const result = await runRenderer(actionFile, "createTodo", "title=Form+Todo")

      expect(result.ok).toBe(true)
      expect(JSON.parse(result.body).data.title).toBe("Form Todo")
    })
  })

  it("returns 404 when an exported action is missing", async () => {
    await withFixture(async ({ actionFile }) => {
      const result = await runRenderer(actionFile, "missingAction", "{}")

      expect(result.ok).toBe(true)
      expect(result.status).toBe(404)
    })
  })
})

async function withFixture(run) {
  const root = await mkdtemp(path.join(exampleRoot, ".ruvyxa-action-test-"))
  const appDir = path.join(root, "app", "todos")
  const actionFile = path.join(appDir, "action.ts")

  await mkdir(appDir, { recursive: true })
  await writeFile(
    actionFile,
    `
      import { action } from "ruvyxa/server"

      export const createTodo = action
        .input({
          parse(value) {
            return { title: String(value.title).trim() }
          },
        })
        .handler(async ({ input, invalidate }) => {
          invalidate("todos")
          return { title: input.title, completed: false }
        })
    `,
  )

  try {
    await run({ root, actionFile })
  } finally {
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  }
}

async function runRenderer(actionFile, actionName, payload) {
  const { stdout } = await execFileAsync(
    "node",
    [renderer, exampleRoot, actionFile, actionName, payload, "/todos"],
    {
      cwd: workspaceRoot,
      maxBuffer: 10 * 1024 * 1024,
    },
  )

  return JSON.parse(stdout)
}
