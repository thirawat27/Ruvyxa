import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises"
import path from "node:path"
import { createInterface } from "node:readline/promises"
import { describe, it } from "node:test"
import { fileURLToPath, pathToFileURL } from "node:url"

import { compileBundle, toImportPath } from "../../../packages/ruvyxa/runtime/compiler.mjs"

const workspaceRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)))
const exampleRoot = path.join(workspaceRoot, "examples/kitchen-sink")
const configRenderer = path.join(workspaceRoot, "packages/ruvyxa/runtime/config-renderer.mjs")
const pluginRunner = path.join(workspaceRoot, "packages/ruvyxa/runtime/plugin-runner.mjs")

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

  it("loads config plugin metadata and executes transform hooks", async () => {
    await withFixture(async ({ root }) => {
      const pageFile = path.join(root, "page.tsx")
      await writeFile(pageFile, "export const label = \"Original\"\n")
      await writeFile(
        path.join(root, "ruvyxa.config.ts"),
        `
          import { defineConfig } from "ruvyxa/config"

          export default defineConfig({
            plugins: [
              {
                name: "replace-label",
                transform(code, id, ctx) {
                  if (ctx.environment !== "client" || !id.endsWith("page.tsx")) return null
                  return { code: code.replace("Original", "Transformed") }
                },
              },
            ],
          })
        `,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.equal(config.ok, true)
      assert.equal(config.config.plugins[0].name, "replace-label")
      assert.equal(config.config.plugins[0].transform, true)

      const transformed = await runJson(pluginRunner, [root, "transform"], {
        code: await readFile(pageFile, "utf8"),
        id: pageFile,
        environment: "client",
      })

      assert.equal(transformed.ok, true)
      assert.match(transformed.result.code, /Transformed/)
    })
  })

  it("serves multiple hooks through persistent plugin runner mode", async () => {
    await withFixture(async ({ root }) => {
      const pageFile = path.join(root, "page.tsx")
      await writeFile(pageFile, "export const label = \"Before\"\n")
      await writeFile(
        path.join(root, "ruvyxa.config.ts"),
        `
          import { defineConfig, plugin } from "ruvyxa/config"

          export default defineConfig({
            plugins: [
              plugin("persistent-replace", {
                resolveId(id) {
                  if (id === "$page") return ${JSON.stringify(pageFile.replaceAll("\\", "/"))}
                  return null
                },
                transform(code, id) {
                  if (!id.endsWith("page.tsx")) return null
                  return code.replace("Before", "After")
                },
              }),
            ],
          })
        `,
      )

      const results = await runPersistentJson(pluginRunner, [root, "--persistent"], [
        {
          hook: "resolveId",
          payload: {
            id: "$page",
            environment: "client",
          },
        },
        {
          hook: "transform",
          payload: {
            code: await readFile(pageFile, "utf8"),
            id: pageFile,
            environment: "client",
          },
        },
      ])

      assert.equal(results[0].ok, true)
      assert.equal(results[0].result, pageFile.replaceAll("\\", "/"))
      assert.equal(results[1].ok, true)
      assert.match(results[1].result.code, /After/)
    })
  })

  it("loads plugin factories and passes stable hook context", async () => {
    await withFixture(async ({ root }) => {
      const pageFile = path.join(root, "page.tsx")
      await writeFile(pageFile, "export const label = \"Before\"\n")
      await writeFile(
        path.join(root, "ruvyxa.config.ts"),
        `
          import { defineConfig, definePlugin } from "ruvyxa/config"

          const replaceLabel = definePlugin(({ root }) => ({
            name: "factory-replace",
            timeoutMs: 1000,
            transform(code, id, ctx) {
              if (ctx.root !== root || ctx.environment !== "client" || ctx.id !== id) return null
              return code.replace("Before", "After")
            },
          }))

          export default defineConfig({
            plugins: [replaceLabel, false, null],
          })
        `,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.equal(config.ok, true)
      assert.equal(config.config.plugins[0].name, "factory-replace")
      assert.equal(config.config.plugins[0].transform, true)

      const transformed = await runJson(pluginRunner, [root, "transform"], {
        code: await readFile(pageFile, "utf8"),
        id: pageFile,
        environment: "client",
      })

      assert.equal(transformed.ok, true)
      assert.match(transformed.result.code, /After/)
    })
  })

  it("loads concise plugin shorthand", async () => {
    await withFixture(async ({ root }) => {
      const pageFile = path.join(root, "page.tsx")
      await writeFile(pageFile, "export const label = \"Before\"\n")
      await writeFile(
        path.join(root, "ruvyxa.config.ts"),
        `
          import { defineConfig, plugin } from "ruvyxa/config"

          export default defineConfig({
            plugins: [
              plugin("short-replace", (code, id) => {
                if (!id.endsWith("page.tsx")) return null
                return code.replace("Before", "After")
              }),
            ],
          })
        `,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.equal(config.ok, true)
      assert.equal(config.config.plugins[0].name, "short-replace")
      assert.equal(config.config.plugins[0].transform, true)

      const transformed = await runJson(pluginRunner, [root, "transform"], {
        code: await readFile(pageFile, "utf8"),
        id: pageFile,
        environment: "client",
      })

      assert.equal(transformed.ok, true)
      assert.match(transformed.result.code, /After/)
    })
  })

  it("loads plugin packages from string names", async () => {
    await withFixture(async ({ root }) => {
      const pageFile = path.join(root, "page.tsx")
      const pluginDir = path.join(root, "node_modules", "ruvyxa-plugin-auto-replace")
      await mkdir(pluginDir, { recursive: true })
      await writeFile(pageFile, "export const label = \"Before\"\n")
      await writeFile(
        path.join(pluginDir, "package.json"),
        JSON.stringify({
          name: "ruvyxa-plugin-auto-replace",
          type: "module",
          main: "./index.mjs",
        }),
      )
      await writeFile(
        path.join(pluginDir, "index.mjs"),
        `
          export default {
            name: "auto-replace",
            transform(code, id) {
              if (!id.endsWith("page.tsx")) return null
              return code.replace("Before", "After")
            },
          }
        `,
      )
      await writeFile(
        path.join(root, "ruvyxa.config.ts"),
        `
          import { defineConfig } from "ruvyxa/config"

          export default defineConfig({
            plugins: ["auto-replace"],
          })
        `,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.equal(config.ok, true)
      assert.equal(config.config.plugins[0].name, "auto-replace")
      assert.equal(config.config.plugins[0].transform, true)

      const transformed = await runJson(pluginRunner, [root, "transform"], {
        code: await readFile(pageFile, "utf8"),
        id: pageFile,
        environment: "client",
      })

      assert.equal(transformed.ok, true)
      assert.match(transformed.result.code, /After/)
    })
  })

  it("reports plugin hook failures with plugin and hook names", async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, "ruvyxa.config.ts"),
        `
          import { defineConfig } from "ruvyxa/config"

          export default defineConfig({
            plugins: [
              {
                name: "broken-transform",
                transform() {
                  throw new Error("intentional failure")
                },
              },
            ],
          })
        `,
      )

      const failed = await runJsonResult(pluginRunner, [root, "transform"], {
        code: "export const label = 'Before'",
        id: path.join(root, "page.tsx"),
        environment: "client",
      })

      assert.notEqual(failed.code, 0)
      assert.equal(failed.parsed.ok, false)
      assert.equal(failed.parsed.code, "RUV1703")
      assert.match(failed.parsed.message, /broken-transform/)
      assert.match(failed.parsed.message, /transform/)
      assert.match(failed.parsed.message, /intentional failure/)
    })
  })

  it("times out long-running plugin hooks", async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, "ruvyxa.config.ts"),
        `
          import { defineConfig } from "ruvyxa/config"

          export default defineConfig({
            plugins: [
              {
                name: "stalled-transform",
                timeoutMs: 5,
                async transform() {
                  await new Promise(() => {})
                },
              },
            ],
          })
        `,
      )

      const failed = await runJsonResult(pluginRunner, [root, "transform"], {
        code: "export const label = 'Before'",
        id: path.join(root, "page.tsx"),
        environment: "client",
      })

      assert.notEqual(failed.code, 0)
      assert.equal(failed.parsed.ok, false)
      assert.equal(failed.parsed.code, "RUV1703")
      assert.match(failed.parsed.message, /stalled-transform/)
      assert.match(failed.parsed.message, /timed out after 5ms/)
    })
  })
})

function runJson(script, args, payload) {
  return runJsonResult(script, args, payload).then((result) => {
    if (result.code === 0 && result.parsed.ok) {
      return result.parsed
    }
    throw new Error(`script failed (${result.code}): ${result.stdout || result.stderr}`)
  })
}

function runJsonResult(script, args, payload) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, ...args], {
      stdio: ["pipe", "pipe", "pipe"],
    })
    let stdout = ""
    let stderr = ""
    child.stdout.setEncoding("utf8")
    child.stderr.setEncoding("utf8")
    child.stdout.on("data", (chunk) => {
      stdout += chunk
    })
    child.stderr.on("data", (chunk) => {
      stderr += chunk
    })
    child.on("error", reject)
    child.on("close", (code) => {
      try {
        const parsed = JSON.parse(stdout)
        resolve({ code, parsed, stdout, stderr })
      } catch (error) {
        reject(new Error(`invalid JSON from script: ${error.message}; stdout=${stdout}; stderr=${stderr}`))
      }
    })
    child.stdin.end(JSON.stringify(payload))
  })
}

async function runPersistentJson(script, args, requests) {
  const child = spawn(process.execPath, [script, ...args], {
    stdio: ["pipe", "pipe", "pipe"],
  })
  const lines = createInterface({
    input: child.stdout,
    crlfDelay: Infinity,
  })
  const lineIterator = lines[Symbol.asyncIterator]()
  let stderr = ""
  child.stderr.setEncoding("utf8")
  child.stderr.on("data", (chunk) => {
    stderr += chunk
  })

  try {
    const results = []
    for (const request of requests) {
      child.stdin.write(`${JSON.stringify(request)}\n`)
      const line = await lineIterator.next()
      if (line.done) {
        throw new Error(`persistent plugin runner exited early; stderr=${stderr}`)
      }
      results.push(JSON.parse(line.value))
    }

    child.stdin.end()
    await new Promise((resolve, reject) => {
      child.on("error", reject)
      child.on("close", (code) => {
        if (code === 0) resolve()
        else reject(new Error(`persistent plugin runner failed (${code}); stderr=${stderr}`))
      })
    })

    return results
  } finally {
    lines.close()
    if (!child.killed) {
      child.kill()
    }
  }
}

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
