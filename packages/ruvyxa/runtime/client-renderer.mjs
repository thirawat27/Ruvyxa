#!/usr/bin/env node
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import {
  cacheFileName,
  collectLayouts,
  compileBundle,
  runtimeAliases,
  toImportPath,
} from './compiler.mjs'

const [projectRootArg, appDirArg, pageFileArg, requestPath = '/', paramsJson = '{}'] =
  process.argv.slice(2)

if (!projectRootArg || !appDirArg || !pageFileArg) {
  fail('RUV1301', 'Client renderer requires projectRoot, appDir, and pageFile arguments.')
}

const projectRoot = path.resolve(projectRootArg)
const appDir = path.resolve(appDirArg)
const pageFile = path.resolve(pageFileArg)

try {
  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const bundleFile = await bundleClientModule(
    projectRoot,
    pageFile,
    layouts,
    requestPath,
    paramsJson,
  )
  const script = await readFile(bundleFile, 'utf8')
  process.stdout.write(JSON.stringify({ ok: true, script }))
} catch (error) {
  fail('RUV1300', error instanceof Error ? error.message : String(error), error?.stack)
}

async function bundleClientModule(projectRoot, pageFile, layouts, requestPath, paramsJson) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'client')
  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []

  layouts.forEach((layoutFile, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(`Layout${index}`)
  })

  const moduleCode = `
import React from "react"
import { hydrateRoot } from "react-dom/client"
${imports.join('\n')}

const params = globalThis.__RUVYXA_ROUTE_PARAMS__ ?? ${paramsJson}
const currentRequestPath = globalThis.__RUVYXA_REQUEST_PATH__ ?? ${JSON.stringify(requestPath)}
let tree = React.createElement(Page, { params, requestPath: currentRequestPath })
for (const Layout of [${wrappers.join(', ')}].reverse()) {
  tree = React.createElement(Layout, null, tree)
}

if (globalThis.__RUVYXA_ROOT__) {
  globalThis.__RUVYXA_ROOT__.render(tree)
} else {
  globalThis.__RUVYXA_ROOT__ = hydrateRoot(document, tree)
}
window.__RUVYXA_HYDRATED = true
`

  const outfile = path.join(cacheDir, cacheFileName([moduleCode, pageFile], 'js'))

  await compileBundle({
    projectRoot,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:client-entry.tsx',
    outfile,
    platform: 'browser',
    minify: process.env.RUVYXA_CLIENT_MINIFY === '1',
    aliases: runtimeAliases(path.dirname(fileURLToPath(import.meta.url))),
  })

  return outfile
}

function fail(code, message, stack) {
  process.stdout.write(JSON.stringify({ ok: false, code, message, stack }))
  process.exit(1)
}
