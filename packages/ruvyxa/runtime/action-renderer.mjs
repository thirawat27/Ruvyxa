#!/usr/bin/env node
import { createHash } from "node:crypto"
import { mkdir } from "node:fs/promises"
import path from "node:path"
import { pathToFileURL } from "node:url"

import { build } from "esbuild"

const [
  projectRootArg,
  actionFileArg,
  actionName = "",
  payloadJson = "{}",
  requestPath = "/",
] = process.argv.slice(2)

if (!projectRootArg || !actionFileArg || !actionName) {
  fail("RUV1503", "Action renderer requires projectRoot, actionFile, and actionName arguments.")
}

const projectRoot = path.resolve(projectRootArg)
const actionFile = path.resolve(actionFileArg)

try {
  const bundleFile = await bundleActionModule(projectRoot, actionFile)
  const mod = await import(pathToFileURL(bundleFile).href + `?t=${Date.now()}`)
  const action = mod[actionName]

  if (typeof action !== "function" || action.ruvyxa?.kind !== "action") {
    process.stdout.write(
      JSON.stringify({
        ok: true,
        status: 404,
        headers: { "content-type": "application/json; charset=utf-8" },
        body: JSON.stringify({
          error: `Action ${actionName} was not found in ${path.basename(actionFile)}`,
        }),
      }),
    )
    process.exit(0)
  }

  const input = parsePayload(payloadJson)
  const invalidated = []
  const request = new Request(`http://localhost${requestPath}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  })
  const result = await action(input, {
    request,
    invalidate(key) {
      invalidated.push(key)
    },
  })
  const response = normalizeActionResult(result, invalidated)
  const body = await response.text()
  const headers = Object.fromEntries(response.headers.entries())

  process.stdout.write(
    JSON.stringify({
      ok: true,
      status: response.status,
      headers,
      body,
    }),
  )
} catch (error) {
  fail("RUV1500", error instanceof Error ? error.message : String(error), error?.stack)
}

async function bundleActionModule(projectRoot, actionFile) {
  const cacheDir = path.join(projectRoot, ".ruvyxa", "cache", "actions")
  await mkdir(cacheDir, { recursive: true })

  const moduleCode = `export * from ${JSON.stringify(toImportPath(actionFile))}`
  const hash = createHash("sha256")
    .update(moduleCode)
    .update(actionFile)
    .digest("hex")
    .slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.mjs`)

  await build({
    stdin: {
      contents: moduleCode,
      resolveDir: projectRoot,
      sourcefile: "ruvyxa:action-entry.ts",
      loader: "ts",
    },
    outfile,
    bundle: true,
    format: "esm",
    platform: "node",
    absWorkingDir: projectRoot,
  })

  return outfile
}

function parsePayload(payloadJson) {
  let parsed
  try {
    parsed = JSON.parse(payloadJson || "{}")
  } catch {
    parsed = Object.fromEntries(new URLSearchParams(payloadJson))
  }

  if (parsed && typeof parsed === "object" && "input" in parsed) {
    return parsed.input
  }
  return parsed
}

function normalizeActionResult(result, invalidated) {
  if (result instanceof Response) {
    return result
  }

  return Response.json({ data: result, invalidated })
}

function toImportPath(file) {
  return path.resolve(file).replaceAll("\\", "/")
}

function fail(code, message, stack) {
  process.stdout.write(JSON.stringify({ ok: false, code, message, stack }))
  process.exit(1)
}
