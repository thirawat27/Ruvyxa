import { execFile } from "node:child_process"
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { promisify } from "node:util"

import { describe, expect, it } from "vitest"

const execFileAsync = promisify(execFile)
const workspaceRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)))
const exampleRoot = path.join(workspaceRoot, "examples/basic-app")
const renderer = path.join(workspaceRoot, "packages/ruvyxa/runtime/client-renderer.mjs")

describe("client renderer boundary diagnostics", () => {
  it("bundles a browser hydration script for a clean page", async () => {
    await withFixture(async ({ appDir, pageFile }) => {
      const result = await runRenderer(appDir, pageFile)
      expect(result.ok).toBe(true)
      expect(result.script).toContain("hydrateRoot")
      expect(result.script).toContain("__RUVYXA_HYDRATED")
    })
  })

  it("blocks server-only marker imports from the client bundle", async () => {
    await withFixture(async ({ appDir, pageFile }) => {
      await writeFile(pageFile, 'import "server-only"\nexport default function Page() { return <main /> }\n')

      const result = await runRenderer(appDir, pageFile, { reject: false })
      expect(result.ok).toBe(false)
      expect(result.message).toContain("RUV1007")
    })
  })

  it("blocks private environment variables from the client bundle", async () => {
    await withFixture(async ({ appDir, pageFile }) => {
      await writeFile(
        pageFile,
        'export default function Page() { return <main>{process.env.DATABASE_URL}</main> }\n',
      )

      const result = await runRenderer(appDir, pageFile, { reject: false })
      expect(result.ok).toBe(false)
      expect(result.message).toContain("RUV1008")
    })
  })
})

async function withFixture(run) {
  const root = await mkdtemp(path.join(exampleRoot, ".ruvyxa-test-"))
  const appDir = path.join(root, "app")
  const pageFile = path.join(appDir, "page.tsx")

  await mkdir(appDir, { recursive: true })
  await writeFile(
    path.join(appDir, "layout.tsx"),
    "export default function Layout({ children }) { return <html><body>{children}</body></html> }\n",
  )
  await writeFile(pageFile, "export default function Page() { return <main>Hello</main> }\n")

  try {
    await run({ root, appDir, pageFile })
  } finally {
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  }
}

async function runRenderer(appDir, pageFile, options = {}) {
  try {
    const { stdout } = await execFileAsync(
      "node",
      [renderer, exampleRoot, appDir, pageFile, "/", "{}"],
      {
        cwd: workspaceRoot,
        maxBuffer: 10 * 1024 * 1024,
      },
    )
    return JSON.parse(stdout)
  } catch (error) {
    if (options.reject === false && error.stdout) {
      return JSON.parse(error.stdout)
    }

    throw error
  }
}
