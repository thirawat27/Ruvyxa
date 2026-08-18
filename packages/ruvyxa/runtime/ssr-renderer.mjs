#!/usr/bin/env node
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  cacheFileName,
  collectLayouts,
  collectSpecials,
  compileBundle,
  runtimeAliases,
  serverPlatform,
  toImportPath,
} from './compiler.mjs'
import { metaSourceImports, nodeSsrEntrySource } from './entry-templates.mjs'
import { requestContext, runWithRequestContext } from './request-context.mjs'

const [projectRootArg, appDirArg, pageFileArg, requestPath = '/', paramsJson = '{}', routePathArg] =
  process.argv.slice(2)

if (!projectRootArg || !appDirArg || !pageFileArg) {
  await fail('RUV1101', 'SSR renderer requires projectRoot, appDir, and pageFile arguments.')
}

const projectRoot = path.resolve(projectRootArg)
const appDir = path.resolve(appDirArg)
const pageFile = path.resolve(pageFileArg)

try {
  const requireFromProject = createRequire(path.join(projectRoot, 'package.json'))
  requireFromProject.resolve('react')
  requireFromProject.resolve('react-dom/server')

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))
  const bundleFile = await bundleSsrModule(
    projectRoot,
    pageFile,
    layouts,
    routePathArg || requestPath,
    specials,
  )
  const mod = await import(pathToFileURL(bundleFile).href + `?t=${Date.now()}`)
  if (typeof mod.render !== 'function') {
    const available = Object.keys(mod).filter((k) => typeof mod[k] === 'function')
    await fail(
      'RUV1100',
      `mod.render is not a function. The bundled module exports: [${available.join(', ') || 'none'}]. ` +
        `This usually indicates a bundler or linker failure for ${pageFileArg}.`,
    )
    process.exit(1)
  }
  // A context with no headers, so `cookies()` and `headers()` return empty
  // rather than throwing. This renderer is driven by `ruvyxa test:parity` and
  // one-shot tooling, which have no HTTP request to forward.
  const html = await runWithRequestContext(
    requestContext({ headerPairs: [], method: 'GET', url: requestPath }),
    () => mod.render({ path: requestPath, params: JSON.parse(paramsJson) }),
  )

  await writeResponse({ ok: true, html })
  process.exit(0)
} catch (error) {
  await fail('RUV1100', error instanceof Error ? error.message : String(error), error?.stack)
}

async function bundleSsrModule(projectRoot, pageFile, layouts, routePath, specials = null) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'ssr')
  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []

  layouts.forEach((layoutFile, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(`Layout${index}`)
  })

  const names = { errorName: null, loadingName: null, notFoundName: null }
  for (const [kind, ident, nameKey] of [
    ['error', 'RouteError', 'errorName'],
    ['loading', 'RouteLoading', 'loadingName'],
    ['notFound', 'RouteNotFound', 'notFoundName'],
  ]) {
    if (specials?.[kind]) {
      imports.push(`import ${ident} from ${JSON.stringify(toImportPath(specials[kind]))}`)
      names[nameKey] = ident
    }
  }

  const { imports: metaImports, metaNames } = metaSourceImports(
    [...layouts, pageFile].map(toImportPath),
  )
  imports.push(...metaImports)

  const moduleCode = nodeSsrEntrySource({
    imports,
    pageName: 'Page',
    layoutNames: wrappers,
    routePath,
    readyEvent: 'onAllReady',
    tolerateStreamErrors: true,
    metaNames,
    ...names,
  })

  const outfile = path.join(cacheDir, cacheFileName([moduleCode, pageFile], 'mjs'))

  await compileBundle({
    projectRoot,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:ssr-entry.tsx',
    outfile,
    platform: serverPlatform(),
    external: ['react', 'react/jsx-runtime', 'react-dom/server', 'node:stream'],
    aliases: runtimeAliases(path.dirname(fileURLToPath(import.meta.url))),
  })

  return outfile
}

async function fail(code, message, stack) {
  try {
    await writeResponse({ ok: false, code, message, stack })
  } finally {
    process.exit(1)
  }
}

function writeResponse(payload) {
  return new Promise((resolve, reject) => {
    process.stdout.write(JSON.stringify(payload), (error) => (error ? reject(error) : resolve()))
  })
}
