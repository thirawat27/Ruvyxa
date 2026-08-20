import type {
  PluginBuildDefinition,
  PluginHeadEntry,
  PluginDevDefinition,
  PluginDiagnostic,
  PluginHttpDefinition,
  PluginNativeDefinition,
  PluginRegistrationApi,
  RuvyxaPlugin,
  RuvyxaPluginDefinition,
} from './types.js'

export type {
  PluginBuildCompleteHook,
  PluginHeadEntry,
  PluginHostEnvironment,
  PluginBuildContext,
  PluginBuildDefinition,
  PluginBuildLoadContext,
  PluginBuildLoadHandler,
  PluginBuildResolveContext,
  PluginBuildResolveHandler,
  PluginBuildSocket,
  PluginBuildStartContext,
  PluginBuildStartHook,
  PluginBuildTransformContext,
  PluginBuildTransformHandler,
  PluginDevFileChangeContext,
  PluginDevFileChangeHandler,
  PluginDevFileChangeRegistration,
  PluginDevDefinition,
  PluginDevSocket,
  PluginDiagnostic,
  PluginDiagnosticLevel,
  PluginDiagnosticsSocket,
  PluginEnvironment,
  PluginHttpContext,
  PluginHttpDefinition,
  PluginHttpRequestContext,
  PluginHttpRequestHandler,
  PluginHttpRequestRegistration,
  PluginHttpResponseContext,
  PluginHttpResponseHandler,
  PluginHttpResponseRegistration,
  PluginHttpRouteContext,
  PluginHttpRouteRegistration,
  PluginHttpSocket,
  PluginNativeCapability,
  PluginNativeDefinition,
  PluginNativeSocket,
  PluginRegistrationApi,
  PluginRoutePattern,
  PluginTransformContext,
  PresencePluginOptions,
  RealtimePluginOptions,
  RuvyxaPlugin,
  RuvyxaPluginDefinition,
  TransformResult,
} from './types.js'

/** Define a plugin through concise declarations or the advanced socket API. */
export function definePlugin(definition: RuvyxaPluginDefinition): RuvyxaPlugin {
  if (!definition || typeof definition !== 'object') {
    throw new TypeError('RUV2102 Ruvyxa plugin must be an object.')
  }
  if (typeof definition.name !== 'string' || definition.name.trim() === '') {
    throw new TypeError('RUV2102 Ruvyxa plugin must have a non-empty name.')
  }
  if (definition.register !== undefined && typeof definition.register !== 'function') {
    throw new TypeError(`RUV2102 Ruvyxa plugin "${definition.name}" register must be a function.`)
  }

  const headers = normalizeHeaders(definition.headers)
  const http = normalizeHttp(definition.http, definition.name, headers !== undefined)
  const build = normalizeBuild(definition.build, definition.name)
  const dev = normalizeDev(definition.dev, definition.name)
  const diagnostics = normalizeDiagnostics(definition.diagnostics)
  const native = normalizeNative(definition.native, definition.name)
  const head = normalizeHead(definition.head, definition.name)
  if (
    !definition.register &&
    !headers &&
    !http &&
    !build &&
    !dev &&
    !diagnostics &&
    !native &&
    !head
  ) {
    throw new TypeError(
      `RUV2102 Ruvyxa plugin "${definition.name}" must declare behavior or provide register(api).`,
    )
  }

  return Object.freeze({
    name: definition.name.trim(),
    ...(head ? { head } : {}),
    register(api: PluginRegistrationApi) {
      registerHttp(api, http, headers)
      registerBuild(api, build)
      if (dev?.onFileChange) api.dev.onFileChange(dev.onFileChange)
      for (const diagnostic of diagnostics ?? []) api.diagnostics.report(diagnostic)
      if (native?.realtime) {
        api.native.claim('realtime@1', native.realtime === true ? {} : native.realtime)
      }
      if (native?.presence) {
        api.native.claim('presence@1', native.presence === true ? {} : native.presence)
      }
      return definition.register?.(api)
    },
  })
}

function registerHttp(
  api: PluginRegistrationApi,
  http: PluginHttpDefinition | undefined,
  headers: readonly [string, string][] | undefined,
): void {
  if (http?.onRequest) api.http.onRequest({ match: http.match, handler: http.onRequest })
  if (http?.onResponse) api.http.onResponse({ match: http.match, handler: http.onResponse })
  for (const route of http?.routes ?? []) api.http.route(route)
  if (headers) {
    api.http.onResponse({
      match: http?.match,
      handler({ response }) {
        return withResponseHeaders(response, headers)
      },
    })
  }
}

function registerBuild(api: PluginRegistrationApi, build: PluginBuildDefinition | undefined): void {
  if (!build) return
  if (build.onStart) api.build.onStart(build.onStart)
  if (build.onResolve) api.build.onResolve(build.onResolve)
  if (build.onLoad) api.build.onLoad(build.onLoad)
  if (build.onTransform) api.build.onTransform(build.onTransform)
  if (build.onComplete) api.build.onComplete(build.onComplete)
}

function normalizeHeaders(
  headers: HeadersInit | undefined,
): readonly [string, string][] | undefined {
  if (headers === undefined) return undefined
  const entries: [string, string][] = []
  new Headers(headers).forEach((value, name) => entries.push([name, value]))
  return entries.length > 0 ? entries : undefined
}

function normalizeHttp(
  http: PluginHttpDefinition | undefined,
  pluginName: string,
  hasGeneratedHeaders: boolean,
): PluginHttpDefinition | undefined {
  if (http === undefined) return undefined
  if (!http || typeof http !== 'object' || Array.isArray(http)) {
    throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" http must be an object.`)
  }
  if (http.onRequest !== undefined && typeof http.onRequest !== 'function') {
    throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" http.onRequest must be a function.`)
  }
  if (http.onResponse !== undefined && typeof http.onResponse !== 'function') {
    throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" http.onResponse must be a function.`)
  }
  if (http.routes !== undefined && !Array.isArray(http.routes)) {
    throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" http.routes must be an array.`)
  }
  const hasBehavior = Boolean(http.onRequest || http.onResponse || (http.routes?.length ?? 0) > 0)
  const scopesGeneratedHeaders = hasGeneratedHeaders && http.match !== undefined
  return hasBehavior || scopesGeneratedHeaders ? http : undefined
}

const BUILD_HOOK_NAMES = new Set(['onStart', 'onResolve', 'onLoad', 'onTransform', 'onComplete'])

function normalizeBuild(
  build: PluginBuildDefinition | undefined,
  pluginName: string,
): PluginBuildDefinition | undefined {
  if (build === undefined) return undefined
  if (!build || typeof build !== 'object' || Array.isArray(build)) {
    throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" build must be an object.`)
  }
  const entries = Object.entries(build)
  if (entries.length === 0) {
    throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" build must declare behavior.`)
  }
  for (const [name, hook] of entries) {
    if (!BUILD_HOOK_NAMES.has(name)) {
      throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" build.${name} is not supported.`)
    }
    if (typeof hook !== 'function') {
      throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" build.${name} must be a function.`)
    }
  }
  return build
}

function normalizeDev(
  dev: PluginDevDefinition | undefined,
  pluginName: string,
): PluginDevDefinition | undefined {
  if (dev === undefined) return undefined
  if (!dev || typeof dev !== 'object' || Array.isArray(dev)) {
    throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" dev must be an object.`)
  }
  if (!dev.onFileChange) {
    throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" dev must declare onFileChange.`)
  }
  return dev
}

function normalizeDiagnostics(
  diagnostics: RuvyxaPluginDefinition['diagnostics'],
): readonly PluginDiagnostic[] | undefined {
  if (diagnostics === undefined) return undefined
  if (Array.isArray(diagnostics)) {
    return diagnostics.length > 0 ? (diagnostics as readonly PluginDiagnostic[]) : undefined
  }
  return [diagnostics as PluginDiagnostic]
}

/** Elements a plugin may contribute to `<head>`, and what may hold text. */
const HEAD_TAGS = new Set(['link', 'meta', 'noscript', 'script', 'style'])
const HEAD_TEXT_TAGS = new Set(['noscript', 'script', 'style'])

/**
 * Validate declared head entries.
 *
 * The tag list is closed on purpose: `<head>` accepts only a handful of
 * elements, and anything else the browser sees there ends the head early and
 * silently moves the rest of the document into `<body>`. Text content is
 * likewise restricted to the raw-text elements, since `<meta>` and `<link>`
 * are void and would drop it.
 */
function normalizeHead(
  head: RuvyxaPluginDefinition['head'],
  pluginName: string,
): readonly PluginHeadEntry[] | undefined {
  if (head === undefined) return undefined
  const entries = Array.isArray(head) ? head : [head as PluginHeadEntry]
  if (entries.length === 0) return undefined

  for (const [index, entry] of entries.entries()) {
    const at = `head[${index}]`
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
      throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" ${at} must be an object.`)
    }
    if (!HEAD_TAGS.has(entry.tag)) {
      throw new TypeError(
        `RUV2102 Ruvyxa plugin "${pluginName}" ${at}.tag must be one of ${[...HEAD_TAGS].join(', ')}.`,
      )
    }
    if (entry.attrs !== undefined) {
      if (!entry.attrs || typeof entry.attrs !== 'object' || Array.isArray(entry.attrs)) {
        throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" ${at}.attrs must be an object.`)
      }
      for (const [name, value] of Object.entries(entry.attrs)) {
        // An attribute name is written unescaped, so it must not be able to
        // introduce another attribute or close the tag.
        if (!/^[A-Za-z][A-Za-z0-9:_.-]*$/.test(name)) {
          throw new TypeError(
            `RUV2102 Ruvyxa plugin "${pluginName}" ${at}.attrs has an invalid attribute name: ${name}.`,
          )
        }
        if (!['string', 'number', 'boolean'].includes(typeof value)) {
          throw new TypeError(
            `RUV2102 Ruvyxa plugin "${pluginName}" ${at}.attrs.${name} must be a string, number, or boolean.`,
          )
        }
      }
    }
    if (entry.children !== undefined) {
      if (typeof entry.children !== 'string') {
        throw new TypeError(
          `RUV2102 Ruvyxa plugin "${pluginName}" ${at}.children must be a string.`,
        )
      }
      if (!HEAD_TEXT_TAGS.has(entry.tag)) {
        throw new TypeError(
          `RUV2102 Ruvyxa plugin "${pluginName}" ${at}.children is only supported on ${[...HEAD_TEXT_TAGS].join(', ')}.`,
        )
      }
      // Raw-text content ends at the matching close tag; a nested one would
      // terminate the element early and inject markup into the document.
      if (new RegExp(`</${entry.tag}`, 'i').test(entry.children)) {
        throw new TypeError(
          `RUV2102 Ruvyxa plugin "${pluginName}" ${at}.children must not contain a closing </${entry.tag}> tag.`,
        )
      }
    }
  }

  return Object.freeze(entries.map((entry) => Object.freeze({ ...entry })))
}

function normalizeNative(
  native: PluginNativeDefinition | undefined,
  pluginName: string,
): PluginNativeDefinition | undefined {
  if (native === undefined) return undefined
  if (!native || typeof native !== 'object' || Array.isArray(native)) {
    throw new TypeError(`RUV2102 Ruvyxa plugin "${pluginName}" native must be an object.`)
  }
  if (!native.realtime && !native.presence) {
    throw new TypeError(
      `RUV2102 Ruvyxa plugin "${pluginName}" native must declare realtime or presence.`,
    )
  }
  return native
}

/** Return a response copy with one header replaced, preserving status and body. */
export function withResponseHeader(response: Response, name: string, value: string): Response {
  return withResponseHeaders(response, [[name, value]])
}

function withResponseHeaders(response: Response, entries: readonly [string, string][]): Response {
  const headers = new Headers(response.headers)
  for (const [name, value] of entries) headers.set(name, value)
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  })
}
