#!/usr/bin/env node
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import { cacheFileName, compileBundle, runtimeAliases, toImportPath } from './compiler.mjs'

const [
  projectRootArg,
  actionFileArg,
  actionName = '',
  payloadJson = '{}',
  requestPath = '/',
  contentType = 'application/json',
] = process.argv.slice(2)

if (!projectRootArg || !actionFileArg || !actionName) {
  fail('RUV1503', 'Action renderer requires projectRoot, actionFile, and actionName arguments.')
}

const projectRoot = path.resolve(projectRootArg)
const actionFile = path.resolve(actionFileArg)

try {
  const bundleFile = await bundleActionModule(projectRoot, actionFile)
  const mod = await import(pathToFileURL(bundleFile).href + `?t=${Date.now()}`)
  const action = mod[actionName]

  if (typeof action !== 'function' || action.ruvyxa?.kind !== 'action') {
    process.stdout.write(
      JSON.stringify({
        ok: true,
        status: 404,
        headers: { 'content-type': 'application/json; charset=utf-8' },
        body: JSON.stringify({
          error: `Action ${actionName} was not found in ${path.basename(actionFile)}`,
        }),
      }),
    )
    process.exit(0)
  }

  const input = parsePayload(payloadJson, contentType)
  const invalidated = []
  const request = new Request(`http://localhost${requestPath}`, {
    method: 'POST',
    headers: { 'content-type': contentType },
    body: contentType === 'application/x-www-form-urlencoded' ? payloadJson : JSON.stringify(input),
  })
  const result = await action(input, {
    request,
    invalidate(key) {
      invalidated.push(key)
    },
  })
  const response = normalizeActionResult(result, invalidated)
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
  fail('RUV1500', error instanceof Error ? error.message : String(error), error?.stack)
}

async function bundleActionModule(projectRoot, actionFile) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'actions')
  const moduleCode = `export * from ${JSON.stringify(toImportPath(actionFile))}`
  const outfile = path.join(cacheDir, cacheFileName([moduleCode, actionFile], 'mjs'))

  await compileBundle({
    projectRoot,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:action-entry.ts',
    outfile,
    platform: 'node',
    aliases: runtimeAliases(path.dirname(fileURLToPath(import.meta.url))),
  })

  return outfile
}

function parsePayload(payloadJson, contentType) {
  const parsed =
    contentType === 'application/x-www-form-urlencoded'
      ? Object.fromEntries(new URLSearchParams(payloadJson || ''))
      : JSON.parse(payloadJson || '{}')

  if (parsed && typeof parsed === 'object' && 'input' in parsed) {
    return parsed.input
  }
  return parsed
}

function normalizeActionResult(result, invalidated) {
  if (result instanceof Response) {
    return result
  }

  return Response.json({ data: result, invalidated })
}

function fail(code, message, stack) {
  process.stdout.write(JSON.stringify({ ok: false, code, message, stack }))
  process.exit(1)
}
