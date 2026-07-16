#!/usr/bin/env node
/**
 * Ruvyxa SSG Renderer — pre-renders pages at build time.
 *
 * Usage:
 *   node ssg-renderer.mjs <projectRoot> <appDir> <pageFile> <requestPath> <mode>
 *
 * Modes:
 *   - "full"   — Full SSG render (complete HTML output)
 *   - "ppr"    — Partial Pre-Rendering (static shell with Suspense boundaries as placeholders)
 *   - "params" — Resolve getStaticParams and return the params array
 *
 * When requestPath is "__resolve_params__", it calls getStaticParams and returns:
 *   { ok: true, params: [...] }
 *
 * Otherwise it renders the page and returns:
 *   { ok: true, html: "..." }
 */
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { Writable } from 'node:stream'

import {
  cacheFileName,
  collectLayouts,
  compileBundle,
  runtimeAliases,
  toImportPath,
} from './ruvyxa-compiler.mjs'

const [
  projectRootArg,
  appDirArg,
  pageFileArg,
  requestPath = '/',
  mode = 'full',
  paramsJson = '{}',
] = process.argv.slice(2)

if (!projectRootArg || !appDirArg || !pageFileArg) {
  fail('RUV1501', 'SSG renderer requires projectRoot, appDir, and pageFile arguments.')
}

const projectRoot = path.resolve(projectRootArg)
const appDir = path.resolve(appDirArg)
const pageFile = path.resolve(pageFileArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))

try {
  const requireFromProject = createRequire(path.join(projectRoot, 'package.json'))
  requireFromProject.resolve('react')
  requireFromProject.resolve('react-dom/server')

  if (requestPath === '__resolve_params__') {
    // Resolve static params mode
    const params = await resolveStaticParams()
    process.stdout.write(JSON.stringify({ ok: true, params }))
  } else {
    // Render mode
    const html = await renderPage(requestPath, mode, parseParams(paramsJson))
    process.stdout.write(JSON.stringify({ ok: true, html }))
  }
} catch (error) {
  fail('RUV1500', error instanceof Error ? error.message : String(error), error?.stack)
}

/**
 * Resolve getStaticParams from the page module.
 */
async function resolveStaticParams() {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'ssg')
  const moduleCode = `export { getStaticParams } from ${JSON.stringify(toImportPath(pageFile))}`

  const outfile = path.join(cacheDir, cacheFileName([moduleCode, pageFile, 'params'], 'mjs'))

  await compileBundle({
    projectRoot,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:ssg-params-entry.ts',
    outfile,
    platform: 'node',
    external: ['react', 'react-dom/server', 'node:stream'],
    aliases: runtimeAliases(runtimeDir),
  })

  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)

  if (typeof mod.getStaticParams !== 'function') {
    return []
  }

  const params = await mod.getStaticParams({ routes: [] })
  return Array.isArray(params) ? params : []
}

/**
 * Render a page to HTML at build time.
 *
 * @param {string} renderPath - The URL path to render
 * @param {string} renderMode - "full" | "ppr"
 */
async function renderPage(renderPath, renderMode, params) {
  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'ssg')

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []

  layouts.forEach((layoutFile, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(`Layout${index}`)
  })

  let moduleCode
  if (renderMode === 'ppr') {
    // PPR mode: render with renderToPipeableStream but only wait for the shell
    // (Suspense boundaries will show their fallback content)
    moduleCode = `
import React from "react"
import { renderToPipeableStream } from "react-dom/server"
import { Writable } from "node:stream"
${imports.join('\n')}

export async function render(ctx) {
  let tree = React.createElement(Page, { params: ctx.params ?? {}, requestPath: ctx.path })
  for (const Layout of [${wrappers.join(', ')}].reverse()) {
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
      onShellReady() {
        // PPR: resolve as soon as the shell is ready (Suspense fallbacks rendered)
        pipe(writable)
        writable.on("finish", () => {
          const html = Buffer.concat(chunks).toString("utf8")
          resolve(html.trimStart().toLowerCase().startsWith("<!doctype") ? html : "<!doctype html>" + html)
        })
      },
      onShellError(error) {
        reject(error)
      },
      onError(error) {
        // Non-fatal streaming errors (dynamic slots will be filled at request time)
      },
    })
  })
}

`
  } else {
    // Full SSG mode: wait for all content to render
    moduleCode = `
import React from "react"
import { renderToPipeableStream } from "react-dom/server"
import { Writable } from "node:stream"
${imports.join('\n')}

export async function render(ctx) {
  let tree = React.createElement(Page, { params: ctx.params ?? {}, requestPath: ctx.path })
  for (const Layout of [${wrappers.join(', ')}].reverse()) {
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
          const html = Buffer.concat(chunks).toString("utf8")
          resolve(html.trimStart().toLowerCase().startsWith("<!doctype") ? html : "<!doctype html>" + html)
        })
      },
      onShellError(error) {
        reject(error)
      },
      onError(error) {
        reject(error)
      },
    })
  })
}
`
  }

  const outfile = path.join(cacheDir, cacheFileName([moduleCode, pageFile, renderPath], 'mjs'))

  await compileBundle({
    projectRoot,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:ssg-entry.tsx',
    outfile,
    platform: 'node',
    external: ['react', 'react-dom/server', 'node:stream'],
    aliases: runtimeAliases(runtimeDir),
  })

  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
  const html = await mod.render({ path: renderPath, params })
  return html
}

function parseParams(paramsJson) {
  const params = JSON.parse(paramsJson)
  if (!params || Array.isArray(params) || typeof params !== 'object') {
    throw new Error('SSG renderer params must be a JSON object.')
  }
  return params
}

function fail(code, message, stack) {
  process.stdout.write(JSON.stringify({ ok: false, code, message, stack }))
  process.exit(1)
}
