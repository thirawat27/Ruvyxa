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

function toImportPath(file) {
  return path.resolve(file).replaceAll("\\", "/")
}

function fail(code, message, stack) {
  process.stdout.write(JSON.stringify({ ok: false, code, message, stack }))
  process.exit(1)
}
