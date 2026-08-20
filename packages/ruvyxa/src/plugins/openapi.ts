import { definePlugin } from '@ruvyxa/core/plugin'
import type { RuvyxaPlugin } from '@ruvyxa/core/plugin'

import { normalizePublicFilePath, writePublicAsset } from './shared.js'

// ─── openApi ─────────────────────────────────────────────────────────────────

export type OpenApiMethod =
  'delete' | 'get' | 'head' | 'options' | 'patch' | 'post' | 'put' | 'trace'

export interface OpenApiOperation {
  method: OpenApiMethod | Uppercase<OpenApiMethod>
  path: string
  summary?: string
  description?: string
  operationId?: string
  tags?: string[]
  parameters?: unknown[]
  requestBody?: Record<string, unknown>
  responses?: Record<string, unknown>
  security?: Array<Record<string, string[]>>
}

export interface OpenApiOptions {
  info: { title: string; version: string; description?: string }
  operations: OpenApiOperation[]
  servers?: Array<{ url: string; description?: string }>
  tags?: Array<{ name: string; description?: string }>
  components?: Record<string, unknown>
  /** @default "/openapi.json" */
  path?: string
}

/** Builds and serves an OpenAPI 3.1 document from explicit API operation metadata. */
export function openApi(options: OpenApiOptions): RuvyxaPlugin {
  if (
    !options?.info ||
    typeof options.info.title !== 'string' ||
    options.info.title.trim() === '' ||
    typeof options.info.version !== 'string' ||
    options.info.version.trim() === ''
  ) {
    throw new TypeError('openApi: info.title and info.version must be non-empty strings')
  }
  if (!Array.isArray(options.operations)) {
    throw new TypeError('openApi: operations must be an array')
  }
  const outputPath = normalizePublicFilePath(options.path ?? '/openapi.json', 'openApi')
  const paths: Record<string, Record<string, unknown>> = {}
  const operationIds = new Set<string>()
  for (const [index, operation] of options.operations.entries()) {
    if (!operation || typeof operation.path !== 'string' || !operation.path.startsWith('/')) {
      throw new TypeError(`openApi: operations[${index}].path must start with "/"`)
    }
    const method = String(operation.method).toLowerCase()
    if (!['delete', 'get', 'head', 'options', 'patch', 'post', 'put', 'trace'].includes(method)) {
      throw new TypeError(`openApi: operations[${index}].method is unsupported`)
    }
    if (paths[operation.path]?.[method]) {
      throw new TypeError(`openApi: duplicate ${method.toUpperCase()} ${operation.path}`)
    }
    if (operation.operationId) {
      if (operationIds.has(operation.operationId)) {
        throw new TypeError(`openApi: duplicate operationId ${operation.operationId}`)
      }
      operationIds.add(operation.operationId)
    }
    paths[operation.path] ??= {}
    paths[operation.path][method] = {
      ...(operation.summary ? { summary: operation.summary } : {}),
      ...(operation.description ? { description: operation.description } : {}),
      ...(operation.operationId ? { operationId: operation.operationId } : {}),
      ...(operation.tags ? { tags: operation.tags } : {}),
      ...(operation.parameters ? { parameters: operation.parameters } : {}),
      ...(operation.requestBody ? { requestBody: operation.requestBody } : {}),
      ...(operation.security ? { security: operation.security } : {}),
      responses: operation.responses ?? { '200': { description: 'Successful response' } },
    }
  }
  const document = {
    openapi: '3.1.0',
    info: options.info,
    ...(options.servers ? { servers: options.servers } : {}),
    ...(options.tags ? { tags: options.tags } : {}),
    paths,
    ...(options.components ? { components: options.components } : {}),
  }
  const body = `${JSON.stringify(document, null, 2)}\n`

  return definePlugin({
    name: 'ruvyxa:openapi',
    register({ http, build }) {
      http.onRequest({
        match: [outputPath],
        handler({ request }) {
          if (new URL(request.url).pathname !== outputPath) return undefined
          return new Response(body, {
            headers: { 'content-type': 'application/json; charset=utf-8' },
          })
        },
      })
      build.onComplete((context) => writePublicAsset(context, outputPath, body))
    },
  })
}
