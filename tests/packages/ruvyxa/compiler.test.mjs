import assert from "node:assert/strict"
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises"
import path from "node:path"
import { describe, it } from "node:test"
import { fileURLToPath, pathToFileURL } from "node:url"

import { compileBundle, toImportPath } from "../../../packages/ruvyxa/runtime/compiler.mjs"

const workspaceRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)))
const exampleRoot = path.join(workspaceRoot, "examples/basic-app")

describe("runtime compiler", () => {
  it("resolves local dynamic imports without an external bundler", async () => {
    await withFixture(async ({ root, outDir }) => {
      await writeFile(path.join(root, "lazy.ts"), "export const value = 42\n")
      const outfile = path.join(outDir, "dynamic.mjs")

      await compileBundle({
        projectRoot: root,
        entrySource: `
          export async function load() {
            const mod = await import("./lazy.js")
            return mod.value
          }
        `,
        sourcefile: "ruvyxa:dynamic-entry.ts",
        outfile,
        platform: "node",
      })

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(await mod.load(), 42)
    })
  })

  it("emits source maps and skips unchanged bundle writes", async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, "page.ts")
      const outfile = path.join(outDir, "mapped.mjs")
      await writeFile(pageFile, "export const answer = 42\n")

      const input = {
        projectRoot: root,
        entrySource: `export * from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: "ruvyxa:mapped-entry.ts",
        outfile,
        platform: "node",
      }

      await compileBundle(input)
      const before = await stat(outfile)
      const map = JSON.parse(await readFile(`${outfile}.map`, "utf8"))
      assert.equal(map.version, 3)
      assert.equal(map.file, path.basename(outfile))
      assert.ok(map.sources.some((source) => source.endsWith("/page.ts")))
      assert.ok(map.sourcesContent.some((source) => source.includes("answer = 42")))

      await new Promise((resolve) => setTimeout(resolve, 25))
      await compileBundle(input)
      const after = await stat(outfile)
      assert.equal(after.mtimeMs, before.mtimeMs)
    })
  })

  it("handles TSX fragments, spread props, and JSX comments", async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, "page.tsx")
      const outfile = path.join(outDir, "jsx.mjs")
      await writeFile(
        pageFile,
        `
          export default function Page(props) {
            return <><main {...props} className="shell">{/* ignored */}<span>{"ok"}</span></main></>
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: "ruvyxa:jsx-entry.tsx",
        outfile,
        platform: "browser",
        external: ["react"],
      })

      const output = await readFile(outfile, "utf8")
      assert.match(output, /React\.Fragment/)
      assert.match(output, /Object\.assign/)
      assert.doesNotMatch(output, /ignored/)
    })
  })

  it("handles JSX returned from ternaries and map callbacks", async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, "page.tsx")
      const outfile = path.join(outDir, "jsx-expressions.mjs")
      await writeFile(
        pageFile,
        `
          export default function Page({ items = ["one"], active = true }) {
            return (
              <main>
                {active ? <strong>Active</strong> : <span>Idle</span>}
                <ul>{items.map((item) => <li key={item}>{item}</li>)}</ul>
              </main>
            )
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: "ruvyxa:jsx-expression-entry.tsx",
        outfile,
        platform: "browser",
        external: ["react"],
      })

      const output = await readFile(outfile, "utf8")
      assert.match(output, /React\.createElement\("strong"/)
      assert.match(output, /items\.map\(\(item\) => React\.createElement\("li"/)
      assert.doesNotMatch(output, /=> <li/)
    })
  })

  it("ignores import, export, and private env examples inside strings", async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, "page.tsx")
      const outfile = path.join(outDir, "string-examples.mjs")
      await writeFile(
        pageFile,
        `
          const snippet = \`
            import secret from "./missing"
            export function POST() {}
            export const createTodo = action
            process.env.DATABASE_URL
          \`

          export default function Page() {
            return <main>{snippet}</main>
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: "ruvyxa:string-example-entry.tsx",
        outfile,
        platform: "browser",
        external: ["react"],
      })

      const output = await readFile(outfile, "utf8")
      assert.match(output, /process\.env\.DATABASE_URL/)
      assert.doesNotMatch(output, /__exports\.POST/)
      assert.doesNotMatch(output, /__exports\.createTodo/)
    })
  })

  it("drops side-effect asset imports from wrapped modules", async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, "page.tsx")
      const outfile = path.join(outDir, "asset-import.mjs")
      await writeFile(path.join(root, "global.css"), "body { margin: 0; }\n")
      await writeFile(
        pageFile,
        `
          import "./global.css"

          export default function Page() {
            return <main>ok</main>
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: "ruvyxa:asset-import-entry.tsx",
        outfile,
        platform: "browser",
        external: ["react"],
      })

      const output = await readFile(outfile, "utf8")
      assert.doesNotMatch(output, /import "\.\/global\.css"/)
    })
  })
})

async function withFixture(run) {
  const root = await mkdtemp(path.join(exampleRoot, ".ruvyxa-compiler-test-"))
  const outDir = path.join(root, ".ruvyxa", "cache")
  await mkdir(outDir, { recursive: true })

  try {
    await run({ root, outDir })
  } finally {
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  }
}
