#!/usr/bin/env node
import { createHash } from "node:crypto"
import { existsSync } from "node:fs"
import { mkdir } from "node:fs/promises"
import { createRequire } from "node:module"
import path from "node:path"
import { pathToFileURL } from "node:url"

import { build } from "esbuild"

const [projectRootArg, appDirArg, pageFileArg, requestPath = "/", paramsJson = "{}"] =
  process.argv.slice(2)

if (!projectRootArg || !appDirArg || !pageFileArg) {
  fail("RUV1101", "SSR renderer requires projectRoot, appDir, and pageFile arguments.")
}

const projectRoot = path.resolve(projectRootArg)
const appDir = path.resolve(appDirArg)
const pageFile = path.resolve(pageFileArg)

try {
  const requireFromProject = createRequire(path.join(projectRoot, "package.json"))
  requireFromProject.resolve("react")
  requireFromProject.resolve("react-dom/server")

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const bundleFile = await bundleSsrModule(projectRoot, pageFile, layouts)
  const mod = await import(pathToFileURL(bundleFile).href + `?t=${Date.now()}`)
  const html = await mod.render({ path: requestPath, params: JSON.parse(paramsJson) })

  process.stdout.write(JSON.stringify({ ok: true, html }))
} catch (error) {
  fail("RUV1100", error instanceof Error ? error.message : String(error), error?.stack)
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
import { renderToString } from "react-dom/server"
${imports.join("\n")}

export async function render(ctx) {
  let tree = React.createElement(Page, { params: ctx.params ?? {}, requestPath: ctx.path })
  for (const Layout of [${wrappers.join(", ")}].reverse()) {
    tree = React.createElement(Layout, null, tree)
  }
  return "<!doctype html>" + renderToString(tree)
}
`

  const hash = createHash("sha256")
    .update(moduleCode)
    .update(pageFile)
    .digest("hex")
    .slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.mjs`)

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
    external: ["react", "react-dom/server"],
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
