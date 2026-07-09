#!/usr/bin/env node
import path from 'node:path'
import { pathToFileURL } from 'node:url'

import { cacheFileName, compileBundle, runtimeAliases, toImportPath } from './compiler.mjs'

const [
  projectRootArg,
  routeFileArg,
  method = 'GET',
  requestPath = '/',
  paramsJson = '{}',
  bodyArg,
  headersJson = '{}',
] = process.argv.slice(2)

if (!projectRootArg || !routeFileArg) {
  fail('RUV1201', 'API renderer requires projectRoot and routeFile arguments.')
}

const projectRoot = path.resolve(projectRootArg)
const routeFile = path.resolve(routeFileArg)

try {
  const bundleFile = await bundleApiModule(projectRoot, routeFile)
  const mod = await import(pathToFileURL(bundleFile).href + `?t=${Date.now()}`)
  const handler = mod[method.toUpperCase()]

  if (typeof handler !== 'function') {
    process.stdout.write(
      JSON.stringify({
        ok: true,
        status: 405,
        headers: { 'content-type': 'text/plain; charset=utf-8' },
        body: `Method ${method.toUpperCase()} is not allowed`,
      }),
    )
    process.exit(0)
  }

  const upperMethod = method.toUpperCase()
  const requestInit = { method: upperMethod, headers: JSON.parse(headersJson) }
  if (bodyArg != null && upperMethod !== 'GET' && upperMethod !== 'HEAD') {
    requestInit.body = bodyArg
  }
  const request = new Request(`http://localhost${requestPath}`, requestInit)
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
  fail('RUV1200', error instanceof Error ? error.message : String(error), error?.stack)
}

async function bundleApiModule(projectRoot, routeFile) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'api')
  const moduleCode = `export * from ${JSON.stringify(toImportPath(routeFile))}`
  const outfile = path.join(cacheDir, cacheFileName([moduleCode, routeFile], 'mjs'))

  await compileBundle({
    projectRoot,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:api-entry.ts',
    outfile,
    platform: 'node',
    aliases: runtimeAliases(path.dirname(new URL(import.meta.url).pathname)),
  })

  return outfile
}

function normalizeResponse(result) {
  if (result instanceof Response) {
    return result
  }

  return Response.json(result)
}

function fail(code, message, stack) {
  process.stdout.write(JSON.stringify({ ok: false, code, message, stack }))
  process.exit(1)
}
