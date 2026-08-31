import { copyFile, mkdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { pathToFileURL } from 'node:url'
import { Readable, Writable } from 'node:stream'

import { repoPath } from './repo-root.ts'

/**
 * Assemble and run the function bundle an adapter's `function` artifact
 * describes.
 *
 * Four adapters write their own platform wrapper — vercel, netlify, firebase,
 * and cloudflare — and each one translates a platform request into a `Request`
 * and a `Response` back into whatever the platform returns. That translation is
 * where the expensive production bugs live: repeated `Set-Cookie`, a body that
 * arrives parsed instead of streamed, binary output, streaming instead of
 * buffering. Only vercel's was ever executed by a test; the other three were
 * held by `assert.match(source, /createHandler/)`, which passes on every one of
 * those defects.
 *
 * The bundle is assembled here rather than in each suite so the four agree on
 * what a deployed function directory contains. `adapter-runner.mjs` is the
 * thing being imitated, and `HANDLER_RUNTIME_FILES` is read from the handler
 * itself — a restated copy passed while a real bundle threw on first request.
 */
const { HANDLER_RUNTIME_FILES: handlerRuntimeFiles } = (await import(
  pathToFileURL(repoPath('packages/ruvyxa/runtime/serverless-handler.mjs')).href
)) as { HANDLER_RUNTIME_FILES: readonly string[] }

export interface DeployedFunctionOptions {
  /** `handlerSource` from the adapter's `function` artifact. */
  handlerSource: string
  /** Route manifest the handler reads, as both JSON and a module. */
  manifest: Record<string, unknown>
  /** Source of the generated `route-modules.mjs` registry. */
  routeModules: string
  /** Extra bundle files, keyed by path relative to the function directory. */
  extraFiles?: Record<string, string>
}

/**
 * Write the function directory, then import it and return its module exports.
 *
 * The manifest is written twice on purpose. The handler reads `manifest.json`
 * from disk on the hosts that keep it, and imports `manifest.mjs` on the ones
 * whose bundler copies only what the module graph reaches.
 */
export async function deployFunction(
  directory: string,
  options: DeployedFunctionOptions,
): Promise<Record<string, unknown>> {
  await mkdir(path.join(directory, 'prerender'), { recursive: true })
  await writeFile(path.join(directory, 'index.mjs'), options.handlerSource)
  await writeFile(path.join(directory, 'manifest.json'), JSON.stringify(options.manifest))
  await writeFile(
    path.join(directory, 'manifest.mjs'),
    `export default ${JSON.stringify(options.manifest)}\n`,
  )
  await writeFile(path.join(directory, 'route-modules.mjs'), options.routeModules)

  for (const [relative, contents] of Object.entries(options.extraFiles ?? {})) {
    await mkdir(path.dirname(path.join(directory, relative)), { recursive: true })
    await writeFile(path.join(directory, relative), contents)
  }

  for (const runtimeFile of handlerRuntimeFiles) {
    await copyFile(
      repoPath('packages/ruvyxa/runtime', runtimeFile),
      path.join(directory, runtimeFile),
    )
  }

  return (await import(
    pathToFileURL(path.join(directory, 'index.mjs')).href + `?t=${Date.now()}`
  )) as Record<string, unknown>
}

/** A manifest with one SSR API route at `/api/echo`. */
export function echoManifest(): Record<string, unknown> {
  return {
    routes: [
      {
        id: 'app/api/echo/route',
        kind: 'api',
        path: '/api/echo',
        file: 'app/api/echo/route.ts',
        render: { strategy: 'ssr' },
      },
    ],
  }
}

/**
 * A route registry whose `/api/echo` handler exercises the three things a
 * platform wrapper gets wrong: it echoes the request body back (so a body that
 * arrived parsed or truncated is visible), it sets two `Set-Cookie` headers (so
 * a wrapper that folds them into one comma-joined value is visible), and it
 * answers `x-binary: 1` with bytes that are not valid UTF-8 (so a wrapper that
 * round-trips the body through a string is visible).
 *
 * `loadActionModule` and `applyPluginHttp` are exported because the generated
 * registry exports them and every wrapper imports all three — a stub that omits
 * them fails at module load rather than at assertion time.
 */
export function echoRouteModules(): string {
  return `const api = {
  async POST({ request }) {
    if (request.headers.get('x-binary') === '1') {
      return new Response(Uint8Array.from([0, 128, 255, 65]), {
        headers: { 'content-type': 'application/octet-stream' },
      })
    }
    const headers = new Headers()
    headers.append('set-cookie', 'first=1; Path=/')
    headers.append('set-cookie', 'second=2; Path=/')
    return new Response(await request.text(), { headers })
  },
}
export async function loadRouteModule() { return api }
export async function loadActionModule() { return null }
export const applyPluginHttp = undefined
// The registry the real build emits also exports the project's ISR store, or
// \`null\` when no \`cache.handler\` is configured. A handler imports it by
// name, so a stub without it fails to load — which is the point.
export const documentCacheHandler = null
`
}

/** The bytes `echoRouteModules` answers `x-binary: 1` with. */
export const ECHO_BINARY_BODY = Buffer.from([0, 128, 255, 65])

/** The two cookies `echoRouteModules` sets on an ordinary echo. */
export const ECHO_COOKIES = ['first=1; Path=/', 'second=2; Path=/']

export interface NodeRequestInit {
  url: string
  method: string
  headers: Record<string, string>
  /** Raw request chunks, as a platform launcher would stream them. */
  chunks?: Buffer[]
  /** A body the platform parsed before the handler ran. */
  body?: unknown
  /** The raw bytes a platform exposes beside its parsed body. */
  rawBody?: Buffer
}

/** A `Readable` shaped like the `IncomingMessage` a Node launcher passes. */
export function nodeRequest(init: NodeRequestInit): Readable {
  const request = Readable.from(init.chunks ?? [])
  return Object.assign(request, {
    url: init.url,
    method: init.method,
    headers: init.headers,
    ...(init.body === undefined ? {} : { body: init.body }),
    ...(init.rawBody === undefined ? {} : { rawBody: init.rawBody }),
  })
}

export interface NodeResponseDouble {
  response: Writable & { statusCode: number; setHeader(name: string, value: unknown): void }
  headers: Map<string, unknown>
  body(): Buffer
}

/**
 * A `ServerResponse` stand-in that is a real `Writable`.
 *
 * A plain object carrying `end()` would accept a wrapper that cannot stream at
 * all, which is the behaviour these tests exist to hold.
 */
export function nodeResponse(): NodeResponseDouble {
  const chunks: Buffer[] = []
  const headers = new Map<string, unknown>()
  const response = Object.assign(
    new Writable({
      write(chunk: Buffer, _encoding: string, callback: () => void) {
        chunks.push(Buffer.from(chunk))
        callback()
      },
    }),
    {
      statusCode: 0,
      setHeader(name: string, value: unknown) {
        headers.set(name, value)
      },
      status(code: number) {
        response.statusCode = code
        return response
      },
    },
  )
  return { response, headers, body: () => Buffer.concat(chunks) }
}
