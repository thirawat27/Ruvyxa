#!/usr/bin/env node
/**
 * Persistent Node worker for Ruvyxa.
 *
 * Stays alive and processes JSON-delimited requests from stdin.
 * Each request line is a JSON object with a `type` field indicating
 * which renderer to invoke. Responses are written as single-line JSON
 * to stdout, terminated by a newline.
 *
 * Protocol:
 *   Request:  { id, type: "ssr"|"api"|"action"|"client", ...args }
 *   Response: { id, ...result }
 *
 * This eliminates the ~100-500ms overhead of spawning node + esbuild
 * per request that the old architecture incurred.
 */
import { createHash } from "node:crypto"
import { existsSync } from "node:fs"
import { mkdir, readFile } from "node:fs/promises"
import { createRequire } from "node:module"
import path from "node:path"
import { pathToFileURL } from "node:url"
import { createInterface } from "node:readline"

import { build } from "esbuild"

// Module-level cache for esbuild bundles (avoids re-bundling unchanged modules)
const bundleCache = new Map()
// Track source file mtimes to invalidate bundles on change
const mtimeCache = new Map()

const rl = createInterface({ input: process.stdin })

rl.on("line", async (line) => {
  let request
  try {
    request = JSON.parse(line)
  } catch {
    return
  }

  const { id } = request
  let result

  try {
    switch (request.type) {
      case "ssr":
        result = await handleSsr(request)
        break
      case "api":
        result = await handleApi(request)
        break
      case "action":
        result = await handleAction(request)
        break
      case "client":
        result = await handleClient(request)
        break
      case "ping":
        result = { ok: true, pong: true }
        break
      case "invalidate":
        invalidateBundleCache(request.paths)
        result = { ok: true }
        break
      default:
        result = { ok: false, code: "RUV1700", message: `Unknown request type: ${request.type}` }
    }
  } catch (error) {
    result = {
      ok: false,
      code: "RUV1700",
      message: error instanceof Error ? error.message : String(error),
      stack: error?.stack,
    }
  }

  result.id = id
  process.stdout.write(JSON.stringify(result) + "\n")
})

rl.on("close", () => {
  process.exit(0)
})

// Keep process alive
process.stdin.resume()

// --- SSR Handler ---
async function handleSsr(request) {
  const { projectRoot, appDir, pageFile, requestPath, params } = request

  const requireFromProject = createRequire(path.join(projectRoot, "package.json"))
  requireFromProject.resolve("react")
  requireFromProject.resolve("react-dom/server")

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const bundleFile = await bundleSsrModule(projectRoot, pageFile, layouts)
  const mod = await import(pathToFileURL(bundleFile).href + `?t=${Date.now()}`)
  const html = await mod.render({ path: requestPath, params: params || {} })

  return { ok: true, html }
}

// --- API Handler ---
async function handleApi(request) {
  const { projectRoot, routeFile, method, requestPath, params } = request

  const bundleFile = await bundleApiModule(projectRoot, routeFile)
  const mod = await import(pathToFileURL(bundleFile).href + `?t=${Date.now()}`)
  const handler = mod[method.toUpperCase()]

  if (typeof handler !== "function") {
    return {
      ok: true,
      status: 405,
      headers: { "content-type": "text/plain; charset=utf-8" },
      body: `Method ${method.toUpperCase()} is not allowed`,
    }
  }

  const req = new Request(`http://localhost${requestPath}`, { method: method.toUpperCase() })
  const result = await handler({ request: req, params: params || {} })
  const response = normalizeResponse(result)
  const body = await response.text()
  const headers = Object.fromEntries(response.headers.entries())

  return { ok: true, status: response.status, headers, body }
}

// --- Action Handler ---
async function handleAction(request) {
  const { projectRoot, actionFile, actionName, payloadJson, requestPath } = request

  const bundleFile = await bundleActionModule(projectRoot, actionFile)
  const mod = await import(pathToFileURL(bundleFile).href + `?t=${Date.now()}`)
  const action = mod[actionName]

  if (typeof action !== "function" || action.ruvyxa?.kind !== "action") {
    return {
      ok: true,
      status: 404,
      headers: { "content-type": "application/json; charset=utf-8" },
      body: JSON.stringify({
        error: `Action ${actionName} was not found in ${path.basename(actionFile)}`,
      }),
    }
  }

  const input = parsePayload(payloadJson)
  const invalidated = []
  const req = new Request(`http://localhost${requestPath}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  })
  const result = await action(input, {
    request: req,
    invalidate(key) {
      invalidated.push(key)
    },
  })
  const response = normalizeActionResult(result, invalidated)
  const body = await response.text()
  const headers = Object.fromEntries(response.headers.entries())

  return { ok: true, status: response.status, headers, body }
}

// --- Client Bundle Handler ---
async function handleClient(request) {
  const { projectRoot, appDir, pageFile, requestPath, params } = request

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const bundleFile = await bundleClientModule(projectRoot, pageFile, layouts, requestPath, JSON.stringify(params || {}))
  const script = await readFile(bundleFile, "utf8")

  return { ok: true, script }
}

// --- Bundle Cache Invalidation ---
function invalidateBundleCache(paths) {
  if (!paths || paths.length === 0) {
    // Full invalidation
    bundleCache.clear()
    mtimeCache.clear()
    return
  }
  // Targeted invalidation: remove entries whose source files match
  for (const [key] of bundleCache) {
    for (const changedPath of paths) {
      const normalized = changedPath.replaceAll("\\", "/")
      if (key.includes(normalized)) {
        bundleCache.delete(key)
        mtimeCache.delete(key)
        break
      }
    }
  }
}

// --- Shared Utilities ---
function collectLayouts(appDir, routeDir) {
  const layouts = []
  let current = appDir

  pushIfExists(layouts, path.join(current, "layout.tsx"))

  const relative = path.relative(appDir, routeDir)
  if (relative && !relative.startsWith("..")) {
    for (const segment of relative.split(path.sep)) {
      if (!segment) continue
      current = path.join(current, segment)
      pushIfExists(layouts, path.join(current, "layout.tsx"))
    }
  }

  return layouts
}

function pushIfExists(collection, file) {
  if (existsSync(file)) {
    collection.push(file)
  }
}

async function bundleSsrModule(projectRoot, pageFile, layouts) {
  const cacheDir = path.join(projectRoot, ".ruvyxa", "cache", "ssr")
  await mkdir(cacheDir, { recursive: true })

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []

  layouts.forEach((layoutFile, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(`Layout${index}`)
  })

  const moduleCode = `
import React from "react"
import { renderToPipeableStream } from "react-dom/server"
import { Writable } from "node:stream"
${imports.join("\n")}

export async function render(ctx) {
  let tree = React.createElement(Page, { params: ctx.params ?? {}, requestPath: ctx.path })
  for (const Layout of [${wrappers.join(", ")}].reverse()) {
    tree = React.createElement(Layout, null, tree)
  }

  return new Promise((resolve, reject) => {
    const chunks = []
    const writable = new Writable({
      write(chunk, encoding, callback) {
        chunks.push(chunk)
        callback()
      },
    })

    const { pipe } = renderToPipeableStream(tree, {
      onAllReady() {
        pipe(writable)
        writable.on("finish", () => {
          resolve("<!doctype html>" + Buffer.concat(chunks).toString("utf8"))
        })
      },
      onShellError(error) {
        reject(error)
      },
      onError(error) {
        // Non-fatal streaming errors are logged but don't abort
        if (process.env.RUVYXA_DEBUG) console.error("[ssr stream error]", error)
      },
    })
  })
}
`

  const hash = createHash("sha256")
    .update(moduleCode)
    .update(pageFile)
    .digest("hex")
    .slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.mjs`)

  const cacheKey = `ssr:${pageFile}:${hash}`
  if (bundleCache.has(cacheKey)) {
    return bundleCache.get(cacheKey)
  }

  await build({
    stdin: {
      contents: moduleCode,
      resolveDir: projectRoot,
      sourcefile: "ruvyxa:ssr-entry.tsx",
      loader: "tsx",
    },
    outfile,
    bundle: true,
    format: "esm",
    platform: "node",
    jsx: "automatic",
    absWorkingDir: projectRoot,
    external: ["react", "react-dom/server", "node:stream"],
    plugins: [
      {
        name: "ruvyxa-css-empty-module",
        setup(build) {
          build.onLoad({ filter: /\.css$/ }, () => ({ contents: "", loader: "js" }))
        },
      },
    ],
  })

  bundleCache.set(cacheKey, outfile)
  return outfile
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

  const cacheKey = `api:${routeFile}:${hash}`
  if (bundleCache.has(cacheKey)) {
    return bundleCache.get(cacheKey)
  }

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

  bundleCache.set(cacheKey, outfile)
  return outfile
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

  const cacheKey = `action:${actionFile}:${hash}`
  if (bundleCache.has(cacheKey)) {
    return bundleCache.get(cacheKey)
  }

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

  bundleCache.set(cacheKey, outfile)
  return outfile
}

async function bundleClientModule(projectRoot, pageFile, layouts, requestPath, paramsJson) {
  const cacheDir = path.join(projectRoot, ".ruvyxa", "cache", "client")
  await mkdir(cacheDir, { recursive: true })

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []

  layouts.forEach((layoutFile, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(`Layout${index}`)
  })

  const moduleCode = `
import React from "react"
import { hydrateRoot } from "react-dom/client"
${imports.join("\n")}

const params = globalThis.__RUVYXA_ROUTE_PARAMS__ ?? ${paramsJson}
const currentRequestPath = globalThis.__RUVYXA_REQUEST_PATH__ ?? ${JSON.stringify(requestPath)}
let tree = React.createElement(Page, { params, requestPath: currentRequestPath })
for (const Layout of [${wrappers.join(", ")}].reverse()) {
  tree = React.createElement(Layout, null, tree)
}

if (globalThis.__RUVYXA_ROOT__) {
  globalThis.__RUVYXA_ROOT__.render(tree)
} else {
  globalThis.__RUVYXA_ROOT__ = hydrateRoot(document, tree)
}
window.__RUVYXA_HYDRATED = true
`

  const hash = createHash("sha256")
    .update(moduleCode)
    .update(pageFile)
    .digest("hex")
    .slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.js`)

  const cacheKey = `client:${pageFile}:${hash}`
  if (bundleCache.has(cacheKey)) {
    return bundleCache.get(cacheKey)
  }

  await build({
    stdin: {
      contents: moduleCode,
      resolveDir: projectRoot,
      sourcefile: "ruvyxa:client-entry.tsx",
      loader: "tsx",
    },
    outfile,
    bundle: true,
    format: "iife",
    platform: "browser",
    jsx: "automatic",
    minify: process.env.RUVYXA_CLIENT_MINIFY === "1",
    treeShaking: true,
    absWorkingDir: projectRoot,
    plugins: [
      clientBoundaryPlugin(projectRoot),
      {
        name: "ruvyxa-css-empty-module",
        setup(build) {
          build.onLoad({ filter: /\.css$/ }, () => ({ contents: "", loader: "js" }))
        },
      },
    ],
  })

  bundleCache.set(cacheKey, outfile)
  return outfile
}

function clientBoundaryPlugin(projectRoot) {
  return {
    name: "ruvyxa-client-boundary",
    setup(build) {
      build.onResolve({ filter: /^server-only$/ }, (args) => {
        throw new Error(
          `[RUV1007] Server-only module imported into client bundle from ${args.importer}`,
        )
      })

      build.onResolve({ filter: /^client-only$/ }, () => ({
        path: "ruvyxa:client-only",
        namespace: "ruvyxa-virtual",
      }))

      build.onLoad({ filter: /^ruvyxa:client-only$/, namespace: "ruvyxa-virtual" }, () => ({
        contents: "",
        loader: "js",
      }))

      build.onLoad({ filter: /\.[cm]?[jt]sx?$/ }, async (args) => {
        if (!isProjectSource(projectRoot, args.path)) {
          return null
        }

        const contents = await readFile(args.path, "utf8")

        if (isServerOnlyPath(projectRoot, args.path)) {
          throw new Error(
            `[RUV1007] Server-only file imported into client bundle: ${args.path}`,
          )
        }

        if (contents.includes('import "server-only"') || contents.includes("import 'server-only'")) {
          throw new Error(
            `[RUV1007] Server-only module imported into client bundle: ${args.path}`,
          )
        }

        const privateEnv = findPrivateEnvAccess(contents)
        if (privateEnv) {
          throw new Error(
            `[RUV1008] Private environment variable ${privateEnv} used in client bundle: ${args.path}`,
          )
        }

        return { contents, loader: loaderForExt(args.path) }
      })
    },
  }
}

function isProjectSource(projectRoot, filePath) {
  const normalized = filePath.replaceAll("\\", "/")
  const normalizedRoot = projectRoot.replaceAll("\\", "/")
  return normalized.startsWith(normalizedRoot) && !normalized.includes("/node_modules/")
}

function isServerOnlyPath(projectRoot, filePath) {
  const relative = path.relative(projectRoot, filePath).replaceAll("\\", "/")
  return relative.split("/").includes("server")
}

function findPrivateEnvAccess(source) {
  const regex = /process\.env\.([A-Z_][A-Z0-9_]*)/g
  let match
  while ((match = regex.exec(source)) !== null) {
    if (!match[1].startsWith("RUVYXA_PUBLIC_")) {
      return match[1]
    }
  }
  return null
}

function loaderForExt(filePath) {
  if (filePath.endsWith(".tsx")) return "tsx"
  if (filePath.endsWith(".ts")) return "ts"
  if (filePath.endsWith(".jsx")) return "jsx"
  return "js"
}

function normalizeResponse(result) {
  if (result instanceof Response) return result
  return Response.json(result)
}

function normalizeActionResult(result, invalidated) {
  if (result instanceof Response) return result
  return Response.json({ data: result, invalidated })
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

function toImportPath(file) {
  return path.resolve(file).replaceAll("\\", "/")
}
