#!/usr/bin/env node
import { createHash } from "node:crypto"
import { mkdir } from "node:fs/promises"
import path from "node:path"
import { pathToFileURL } from "node:url"

import { build } from "esbuild"

const [
  projectRootArg,
  routeFileArg,
  method = "GET",
  requestPath = "/",
  paramsJson = "{}",
] = process.argv.slice(2)

if (!projectRootArg || !routeFileArg) {
  fail("RUV1201", "API renderer requires projectRoot and routeFile arguments.")
}

const projectRoot = path.resolve(projectRootArg)
const routeFile = path.resolve(routeFileArg)

try {
  const bundleFile = await bundleApiModule(projectRoot, routeFile)
  const mod = await import(pathToFileURL(bundleFile).href + `?t=${Date.now()}`)
  const handler = mod[method.toUpperCase()]

  if (typeof handler !== "function") {
    process.stdout.write(
      JSON.stringify({
        ok: true,
        status: 405,
        headers: { "content-type": "text/plain; charset=utf-8" },
        body: `Method ${method.toUpperCase()} is not allowed`,
      }),
    )
    process.exit(0)
  }

  const request = new Request(`http://localhost${requestPath}`, { method: method.toUpperCase() })
  const result = await handler({
    request,
    params: JSON.parse(paramsJson),
  })
  const response = normalizeResponse(result)
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
  fail("RUV1200", error instanceof Error ? error.message : String(error), error?.stack)
}

async function bundleApiModule(projectRoot, routeFile) {
  const cacheDir = path.join(projectRoot, ".ruvyxa", "cache", "api")
  await mkdir(cacheDir, { recursive: true })

  const moduleCode = `export * from ${JSON.stringify(toImportPath(routeFile))}`
  const hash = createHash("sha256")
    .update(moduleCode)
    .update(routeFile)
    .digest("hex")
    .slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.mjs`)

  await build({
    stdin: {
      contents: moduleCode,
      resolveDir: projectRoot,
      sourcefile: "ruvyxa:api-entry.ts",
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

function normalizeResponse(result) {
  if (result instanceof Response) {
    return result
  }

  return Response.json(result)
}

function toImportPath(file) {
  return path.resolve(file).replaceAll("\\", "/")
}

function fail(code, message, stack) {
  process.stdout.write(JSON.stringify({ ok: false, code, message, stack }))
  process.exit(1)
}
