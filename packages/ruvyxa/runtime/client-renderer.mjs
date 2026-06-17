#!/usr/bin/env node
import { createHash } from "node:crypto"
import { existsSync } from "node:fs"
import { mkdir, readFile } from "node:fs/promises"
import path from "node:path"

import { build } from "esbuild"

const [projectRootArg, appDirArg, pageFileArg, requestPath = "/", paramsJson = "{}"] =
  process.argv.slice(2)

if (!projectRootArg || !appDirArg || !pageFileArg) {
  fail("RUV1301", "Client renderer requires projectRoot, appDir, and pageFile arguments.")
}

const projectRoot = path.resolve(projectRootArg)
const appDir = path.resolve(appDirArg)
const pageFile = path.resolve(pageFileArg)

try {
  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const bundleFile = await bundleClientModule(projectRoot, pageFile, layouts, requestPath, paramsJson)
  const script = await readFile(bundleFile, "utf8")
  process.stdout.write(JSON.stringify({ ok: true, script }))
} catch (error) {
  fail("RUV1300", error instanceof Error ? error.message : String(error), error?.stack)
}

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

const params = ${paramsJson}
let tree = React.createElement(Page, { params, requestPath: ${JSON.stringify(requestPath)} })
for (const Layout of [${wrappers.join(", ")}].reverse()) {
  tree = React.createElement(Layout, null, tree)
}

hydrateRoot(document, tree)
window.__RUVYXA_HYDRATED = true
`

  const hash = createHash("sha256")
    .update(moduleCode)
    .update(pageFile)
    .digest("hex")
    .slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.js`)

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

  return outfile
}

function clientBoundaryPlugin(projectRoot) {
  return {
    name: "ruvyxa-client-boundary",
    setup(build) {
      build.onResolve({ filter: /^server-only$/ }, (args) => {
        throw boundaryError(
          "RUV1007",
          "Server-only module imported into client bundle",
          args.importer,
          'Remove `import "server-only"` from code that is reachable by the browser bundle.',
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

        const normalized = args.path.replaceAll("\\", "/")
        const contents = await readFile(args.path, "utf8")

        if (isServerOnlyPath(projectRoot, args.path)) {
          throw boundaryError(
            "RUV1007",
            "Server-only file imported into client bundle",
            args.path,
            "Move this import behind a server loader/API route, or pass serialized data into the page.",
          )
        }

        if (contents.includes('import "server-only"') || contents.includes("import 'server-only'")) {
          throw boundaryError(
            "RUV1007",
            "Server-only module imported into client bundle",
            args.path,
            'Move server-only code out of the browser graph or remove `import "server-only"`.',
          )
        }

        const privateEnv = findPrivateEnvAccess(contents)
        if (privateEnv) {
          throw boundaryError(
            "RUV1008",
            "Private environment variable used in client bundle",
            args.path,
            `Rename ${privateEnv} to RUVYXA_PUBLIC_* if it is safe for browsers, or move the code to server-only logic.`,
          )
        }

        return {
          contents,
          loader: loaderFor(normalized),
        }
      })
    },
  }
}

function isProjectSource(projectRoot, file) {
  const relative = path.relative(projectRoot, file)
  return relative && !relative.startsWith("..") && !path.isAbsolute(relative) && !relative.includes("node_modules")
}

function isServerOnlyPath(projectRoot, file) {
  const normalized = path.relative(projectRoot, file).replaceAll("\\", "/")
  return (
    normalized === "server.ts" ||
    normalized === "server.js" ||
    normalized.startsWith("server/") ||
    normalized.endsWith("/server.ts") ||
    normalized.endsWith("/server.js")
  )
}

function findPrivateEnvAccess(contents) {
  const matches = contents.matchAll(/\bprocess\.env\.([A-Z_][A-Z0-9_]*)/g)

  for (const match of matches) {
    const name = match[1]
    if (name !== "NODE_ENV" && !name.startsWith("RUVYXA_PUBLIC_")) {
      return name
    }
  }

  return null
}

function loaderFor(file) {
  if (file.endsWith(".tsx")) return "tsx"
  if (file.endsWith(".jsx")) return "jsx"
  if (file.endsWith(".ts") || file.endsWith(".mts") || file.endsWith(".cts")) return "ts"
  return "js"
}

function boundaryError(code, title, file, fix) {
  return new Error(`${code}: ${title}

File:
  ${file || "(entrypoint)"}

Why this is a problem:
  This module is reachable from the browser hydration bundle.

Fix:
  ${fix}`)
}

function toImportPath(file) {
  return path.resolve(file).replaceAll("\\", "/")
}

function fail(code, message, stack) {
  process.stdout.write(JSON.stringify({ ok: false, code, message, stack }))
  process.exit(1)
}
