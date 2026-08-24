#!/usr/bin/env node
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  cacheFileName,
  compileBundle,
  runtimeAliases,
  serverPlatform,
  toImportPath,
} from './compiler.mjs'

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
  await fail('RUV1201', 'API renderer requires projectRoot and routeFile arguments.')
}

const projectRoot = path.resolve(projectRootArg)
const routeFile = path.resolve(routeFileArg)

try {
  const bundleFile = await bundleApiModule(projectRoot, routeFile)
  const mod = await import(pathToFileURL(bundleFile).href + `?t=${Date.now()}`)
  const handler = mod[method.toUpperCase()]

  if (typeof handler !== 'function') {
    await writeResponse({
      ok: true,
      status: 405,
      headers: { 'content-type': 'text/plain; charset=utf-8' },
      body: `Method ${method.toUpperCase()} is not allowed`,
    })
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
  const response = normalizeResponse(result, `${method} ${requestPath}`)
  const body = await response.text()
  const headerPairs = responseHeaderPairs(response)
  const headers = Object.fromEntries(headerPairs)

  await writeResponse({
    ok: true,
    status: response.status,
    headers,
    headerPairs,
    body,
  })
  process.exit(0)
} catch (error) {
  await fail('RUV1200', error instanceof Error ? error.message : String(error), error?.stack)
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
    platform: serverPlatform(),
    aliases: runtimeAliases(path.dirname(fileURLToPath(import.meta.url))),
  })

  return outfile
}

function normalizeResponse(result, route = 'this route') {
  if (result instanceof Response) return result
  // Returning serialisable data instead of a Response is a supported
  // convenience. Returning nothing is not: `Response.json(undefined)` throws
  // "Value is not JSON serializable" from inside undici, and the message that
  // reached the caller named neither the handler nor the fact that it returned
  // nothing — the suggested fix was to check the module's imports.
  if (result === undefined) {
    throw new Error(
      `RUV1504 the handler for ${route} returned nothing. A route handler must return a Response, ` +
        'or data that can be serialised as JSON, which is sent as `Response.json(data)`.',
    )
  }
  try {
    return Response.json(result)
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new Error(
      `RUV1504 the handler for ${route} returned a value that cannot be serialised as JSON ` +
        `(${detail}). Return a Response, or data built from plain objects, arrays, strings, ` +
        'numbers, booleans, and null.',
    )
  }
}

function responseHeaderPairs(response) {
  const headerPairs = []
  for (const [name, value] of response.headers.entries()) {
    if (name !== 'set-cookie') headerPairs.push([name, value])
  }
  for (const value of response.headers.getSetCookie()) {
    headerPairs.push(['set-cookie', value])
  }
  return headerPairs
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
