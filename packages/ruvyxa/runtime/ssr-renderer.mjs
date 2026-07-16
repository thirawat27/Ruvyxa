#!/usr/bin/env node
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  cacheFileName,
  collectLayouts,
  compileBundle,
  runtimeAliases,
  toImportPath,
} from './ruvyxa-compiler.mjs'

const [projectRootArg, appDirArg, pageFileArg, requestPath = '/', paramsJson = '{}'] =
  process.argv.slice(2)

if (!projectRootArg || !appDirArg || !pageFileArg) {
  fail('RUV1101', 'SSR renderer requires projectRoot, appDir, and pageFile arguments.')
}

const projectRoot = path.resolve(projectRootArg)
const appDir = path.resolve(appDirArg)
const pageFile = path.resolve(pageFileArg)

try {
  const requireFromProject = createRequire(path.join(projectRoot, 'package.json'))
  requireFromProject.resolve('react')
  requireFromProject.resolve('react-dom/server')

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const bundleFile = await bundleSsrModule(projectRoot, pageFile, layouts)
  const mod = await import(pathToFileURL(bundleFile).href + `?t=${Date.now()}`)
  const html = await mod.render({ path: requestPath, params: JSON.parse(paramsJson) })

  process.stdout.write(JSON.stringify({ ok: true, html }))
} catch (error) {
  fail('RUV1100', error instanceof Error ? error.message : String(error), error?.stack)
}

async function bundleSsrModule(projectRoot, pageFile, layouts) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'ssr')
  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []

  layouts.forEach((layoutFile, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(`Layout${index}`)
  })

  const moduleCode = `
import React from "react"
import { renderToString } from "react-dom/server"
${imports.join('\n')}

export async function render(ctx) {
  let tree = React.createElement(Page, { params: ctx.params ?? {}, requestPath: ctx.path })
  for (const Layout of [${wrappers.join(', ')}].reverse()) {
    tree = React.createElement(Layout, null, tree)
  }
  return "<!doctype html>" + renderToString(tree)
}
`

  const outfile = path.join(cacheDir, cacheFileName([moduleCode, pageFile], 'mjs'))

  await compileBundle({
    projectRoot,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:ssr-entry.tsx',
    outfile,
    platform: 'node',
    external: ['react', 'react-dom/server'],
    aliases: runtimeAliases(path.dirname(fileURLToPath(import.meta.url))),
  })

  return outfile
}

function fail(code, message, stack) {
  process.stdout.write(JSON.stringify({ ok: false, code, message, stack }))
  process.exit(1)
}
