import { existsSync, readFileSync, writeSync } from 'node:fs'
import path from 'node:path'
import { once } from 'node:events'
import { createInterface } from 'node:readline'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  cacheFileName,
  compileBundleIfChanged,
  compileContentSource,
  runtimeAliases,
  serverPlatform,
  toImportPath,
} from './compiler.mjs'
import { transformWithReactCompiler } from './react-compiler.mjs'

/**
 * The JavaScript worker the Rust host spawns for the parts of a project that
 * are code rather than data.
 *
 * `ruvyxa.config.ts` is a TypeScript module. Most of it renders to JSON the
 * CLI reads directly, but three things in it can only run in a JavaScript
 * runtime: `proxy.handler`, the Markdown pipeline (`content.compile`, unified
 * plugins), and the React compiler. The content engine's derived artifacts are
 * the fourth resident, because they are computed from the same project code.
 * One persistent process answers all of them over newline-delimited JSON on
 * stdio; `crates/ruvyxa_middleware/src/worker_host.rs` is the other end.
 */

const [projectRootArg, mode] = process.argv.slice(2)

if (!projectRootArg || !mode) {
  exitWithResponse(
    failure('RUV1701', 'Project worker requires project root and mode arguments.'),
    1,
  )
}

// Stdout is reserved for the NDJSON protocol.
console.log = console.info = console.debug = (...args) => console.error(...args)

const projectRoot = path.resolve(projectRootArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))

/** What the compiled config module gave this process to run. */
const project = {
  /** `proxy` from the config when it carries a handler function. */
  proxy: undefined,
  reactCompiler: false,
  /** The `markdown` block, handed to the MDX compiler as configured. */
  markdown: null,
  /** The content engine `content` declares, or `undefined`. */
  content: undefined,
}

try {
  await loadProject(projectRoot)
  if (mode === '--persistent') {
    await runPersistent()
  } else {
    const payload = JSON.parse(readFileSync(0, 'utf8'))
    const response = await handleHook(mode, payload)
    await writeResponse(response)
    if (!response.ok) process.exitCode = 1
  }
} catch (error) {
  await writeResponse(failureFromError(error), mode === '--persistent')
  process.exitCode = 1
}

async function loadProject(root) {
  const configFile = findConfig(root)
  if (!configFile) return

  const moduleCode = `export { default } from ${JSON.stringify(toImportPath(configFile))}`
  const outfile = path.join(
    root,
    '.ruvyxa',
    'cache',
    'config',
    cacheFileName([moduleCode, configFile, 'project-worker'], 'mjs'),
  )
  await compileBundleIfChanged({
    projectRoot: root,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:project-config-entry.ts',
    outfile,
    platform: serverPlatform(),
    bundleAliasDependencies: true,
    aliases: runtimeAliases(runtimeDir),
    markdownConfig: false,
  })

  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
  const config = mod.default ?? {}
  project.reactCompiler = config.reactCompiler === true
  project.markdown = config.markdown ?? null
  // `proxy.handler` is a function, so it lives here in the compiled config
  // module rather than in the JSON the Rust host reads. The host decides from
  // the matcher which requests to send; this side only runs the handler.
  project.proxy =
    config.proxy && typeof config.proxy.handler === 'function' ? config.proxy : undefined
  project.content = await configuredContentEngine(root, configFile, config)
  if (project.content?.warning) console.warn(`[ruvyxa] ${project.content.warning}`)
}

async function configuredContentEngine(root, configFile, config) {
  const content = config?.content
  const enabled =
    content === true ||
    (content &&
      typeof content === 'object' &&
      !Array.isArray(content) &&
      (content.engine === true ||
        (content.engine && typeof content.engine === 'object' && !Array.isArray(content.engine))))
  if (!enabled) return undefined

  const moduleCode = 'export { createContentEngine as default } from "ruvyxa/content-engine"'
  const outfile = path.join(
    root,
    '.ruvyxa',
    'cache',
    'config',
    cacheFileName([moduleCode, configFile, 'content-engine-runtime'], 'mjs'),
  )
  await compileBundleIfChanged({
    projectRoot: root,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:content-engine-config-entry.ts',
    outfile,
    platform: serverPlatform(),
    bundleAliasDependencies: true,
    aliases: runtimeAliases(runtimeDir),
  })
  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
  return mod.default(config)
}

function findConfig(root) {
  for (const fileName of [
    'ruvyxa.config.ts',
    'ruvyxa.config.mts',
    'ruvyxa.config.js',
    'ruvyxa.config.mjs',
  ]) {
    const file = path.join(root, fileName)
    if (existsSync(file)) return file
  }
  return null
}

async function runPersistent() {
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity })
  for await (const line of lines) {
    if (!line.trim()) continue
    let response
    try {
      const payload = JSON.parse(line)
      response = await handleHook(payload.hook, payload)
    } catch (error) {
      response = failureFromError(error)
    }
    await writeResponse(response, true)
  }
}

async function handleHook(hook, payload) {
  switch (hook) {
    case 'describe':
      return success(describeProject())
    case 'proxy':
      return success(await runProxy(payload))
    case 'build.transform':
      return success(runBuildTransform(payload))
    case 'content.compile':
      return success(await runContentCompile(payload))
    case 'content.artifact':
      return success(contentArtifact(payload))
    case 'content.write':
      writeContentArtifacts(payload)
      return success(null)
    default:
      return failure('RUV1701', `Unknown worker hook: ${hook}`)
  }
}

/**
 * What this process can do for the host, decided once at startup.
 *
 * `content` lists the public paths the content engine answers live, so the
 * host asks about those and no others; `proxy` says whether a handler exists
 * at all, the matcher being the host's own decision.
 */
function describeProject() {
  return {
    proxy: project.proxy !== undefined,
    reactCompiler: project.reactCompiler,
    content: project.content ? [...project.content.paths] : [],
  }
}

/**
 * The React compiler over one module, when the project turned it on.
 *
 * This is a compilation of React components, not a project hook: the
 * JavaScript compiler applies the same transform on its own path.
 */
function runBuildTransform(payload) {
  if (!project.reactCompiler) return null
  const compiled = transformWithReactCompiler(String(payload.code ?? ''), String(payload.id ?? ''))
  if (!compiled) return null
  return { code: compiled.code, ...(compiled.map === undefined ? {} : { map: compiled.map }) }
}

async function runContentCompile(payload) {
  const id = path.resolve(String(payload.id ?? ''))
  const extension = path.extname(id).toLowerCase()
  if (extension !== '.md' && extension !== '.mdx') return null
  const compiled = await compileContentSource(
    String(payload.code ?? ''),
    id,
    projectRoot,
    project.markdown,
  )
  return { code: compiled.source }
}

/** One live content-engine artifact for `ruvyxa dev`, or `null`. */
function contentArtifact(payload) {
  if (!project.content) return null
  const pathname = String(payload.path ?? '')
  return project.content.artifact(projectRoot, pathname) ?? null
}

/** Every content-engine artifact into `<outDir>/assets`, for `ruvyxa build`. */
function writeContentArtifacts(payload) {
  if (!project.content) return
  project.content.write(projectRoot, path.resolve(String(payload.outDir ?? '')))
}

/**
 * Run `proxy.handler` and report what it decided.
 *
 * A `Response` answers the request; a `Request` is the one to continue with —
 * a different path is a rewrite, different headers are forwarded; anything
 * else continues with the request unchanged.
 */
async function runProxy(payload) {
  const request = requestFromPayload(payload.request)
  if (!project.proxy) return { kind: 'request', request: await requestToPayload(request) }
  const result = await project.proxy.handler(request)
  if (result instanceof Response) {
    return { kind: 'response', response: await responseToPayload(result) }
  }
  const next = result instanceof Request ? result : request
  return { kind: 'request', request: await requestToPayload(next) }
}

function requestFromPayload(value = {}) {
  const pathname = typeof value.path === 'string' && value.path.startsWith('/') ? value.path : '/'
  const method = String(value.method ?? 'GET').toUpperCase()
  const body = method === 'GET' || method === 'HEAD' ? undefined : decodeBody(value.bodyBase64)
  const headers = headersFromPairs(value.headers)
  // The request's own `Host`, so a handler reading `request.url` sees the
  // address the client used rather than a placeholder; the placeholder stands
  // in only when the client sent none.
  const host = headers.get('host')?.trim() || 'ruvyxa.local'
  return new Request(`http://${host}${pathname}`, { method, headers, body })
}

async function requestToPayload(request) {
  const url = new URL(request.url)
  return {
    method: request.method,
    path: url.pathname + url.search,
    headers: headerPairs(request.headers),
    bodyBase64: await encodeBody(request),
  }
}

async function responseToPayload(response) {
  return {
    status: response.status,
    headers: headerPairs(response.headers),
    bodyBase64: await encodeBody(response),
  }
}

function headersFromPairs(value) {
  const headers = new Headers()
  if (Array.isArray(value)) {
    for (const pair of value) {
      if (Array.isArray(pair) && pair.length === 2) headers.append(String(pair[0]), String(pair[1]))
    }
  }
  return headers
}

function headerPairs(headers) {
  const pairs = Array.from(headers.entries()).filter(([name]) => name !== 'set-cookie')
  const cookies = typeof headers.getSetCookie === 'function' ? headers.getSetCookie() : []
  for (const cookie of cookies) pairs.push(['set-cookie', cookie])
  return pairs
}

function decodeBody(value) {
  // An empty string is "no body", not "a body of no bytes": the difference
  // decides whether a null-body status can be reconstructed at all.
  if (typeof value !== 'string' || value === '') return undefined
  return Buffer.from(value, 'base64')
}

async function encodeBody(message) {
  const bytes = Buffer.from(await message.arrayBuffer())
  return bytes.length > 0 ? bytes.toString('base64') : undefined
}

function success(result) {
  return { ok: true, result }
}

function failure(code, message, stack) {
  return { ok: false, code, message, stack }
}

function failureFromError(error) {
  return failure('RUV1700', error instanceof Error ? error.message : String(error), error?.stack)
}

/**
 * Write one protocol message, waiting for the pipe when it is full.
 *
 * The persistent mode answers hook after hook down one stdout pipe. Ignoring
 * `write()`'s return value there does not drop anything, but it does let the
 * process buffer every unread response in memory while a slow host reads —
 * unbounded growth on the one path that runs for the life of a dev server.
 * Waiting for `drain` hands the backpressure back, which is what
 * `worker-pool.mjs`'s `writeWorkerMessage` already does. See the
 * stdio-protocol rule in `AGENTS.md`.
 */
async function writeResponse(response, newline = false) {
  if (!process.stdout.write(JSON.stringify(response) + (newline ? '\n' : ''))) {
    await once(process.stdout, 'drain')
  }
}

/**
 * Write a final response and leave, without racing the write against the exit.
 *
 * Stdout here is a pipe read by the Rust host, and a write to a pipe is
 * asynchronous: `process.exit()` tears the process down without draining one
 * that has not flushed, so `writeResponse()` followed by `process.exit(1)`
 * could drop the very diagnostic that explains why the run failed, leaving the
 * host to report unparsable output instead. Writing straight to fd 1 removes
 * the race rather than narrowing it. Every other exit path sets
 * `process.exitCode` and returns, which lets Node drain stdout on its own.
 */
function exitWithResponse(response, code) {
  writeAllSync(1, JSON.stringify(response))
  process.exit(code)
}

/**
 * Write every byte, however many `write(2)` calls that takes.
 *
 * `writeSync` is one `write(2)`, and Node leaves a stdout pipe non-blocking on
 * macOS and Linux: once the pipe buffer is full the call returns a *short
 * count* rather than blocking until the reader drains it. The bytes past that
 * count are never written, and the `process.exit()` above means there is no
 * second chance, so a response larger than one pipe buffer arrives at the Rust
 * host cut mid-JSON and is reported as unparsable output from a run that
 * succeeded. `css-runner.mjs` hit exactly that with a Tailwind stylesheet.
 *
 * The buffer is 64 KiB on Linux and 16 KiB on macOS; Windows writes pipes
 * blocking, so only the first two can lose a tail. See the stdio-protocol rule
 * in `AGENTS.md`.
 */
function writeAllSync(fd, text) {
  const buffer = Buffer.from(text, 'utf8')
  let written = 0
  while (written < buffer.length) {
    try {
      written += writeSync(fd, buffer, written, buffer.length - written)
    } catch (error) {
      // A pipe with no room at all answers `EAGAIN` instead of a short count.
      // The reader has not gone away, it has not caught up — so retry rather
      // than lose the tail. Any other error is real and must not be swallowed.
      if (error.code !== 'EAGAIN') throw error
    }
  }
}
