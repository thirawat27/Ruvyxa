import { createRequire } from "node:module"
import path from "node:path"
import { pathToFileURL } from "node:url"

const DEFAULT_PLUGIN_TIMEOUT_MS = 30_000

export class PluginHookError extends Error {
  constructor(pluginName, hookName, cause) {
    const message = cause instanceof Error ? cause.message : String(cause)
    super(`Plugin '${pluginName}' ${hookName} hook failed: ${message}`)
    this.name = "PluginHookError"
    this.pluginName = pluginName
    this.hookName = hookName
    this.cause = cause
  }
}

export async function normalizePlugins(value, setupContext = {}) {
  if (!Array.isArray(value)) return []

  const plugins = []
  for (const entry of value) {
    await collectPlugin(entry, setupContext, plugins)
  }

  return orderPlugins(plugins)
}

export function pluginDescriptors(plugins) {
  const descriptors = plugins
    .filter(isPluginObject)
    .map((plugin) => ({
      name: plugin.name,
      enforce: stringValue(plugin.enforce),
      resolveId: typeof plugin.resolveId === "function",
      transform: typeof plugin.transform === "function",
    }))

  return descriptors.length > 0 ? descriptors : undefined
}

export async function runPluginHook(plugin, hookName, invoke) {
  const timeoutMs = pluginTimeoutMs(plugin)

  try {
    return await withTimeout(Promise.resolve().then(invoke), timeoutMs, () => {
      throw new Error(`timed out after ${timeoutMs}ms`)
    })
  } catch (error) {
    throw new PluginHookError(plugin.name, hookName, error)
  }
}

function orderPlugins(plugins) {
  return [
    ...plugins.filter((plugin) => plugin.enforce === "pre"),
    ...plugins.filter((plugin) => plugin.enforce !== "pre" && plugin.enforce !== "post"),
    ...plugins.filter((plugin) => plugin.enforce === "post"),
  ]
}

async function collectPlugin(entry, setupContext, plugins) {
  if (!entry) return

  if (Array.isArray(entry)) {
    for (const item of entry) {
      await collectPlugin(item, setupContext, plugins)
    }
    return
  }

  if (typeof entry === "function") {
    const result = await entry(setupContext)
    await collectPlugin(result, setupContext, plugins)
    return
  }

  if (typeof entry === "string") {
    const result = await loadPluginReference(entry, setupContext)
    await collectPlugin(result, setupContext, plugins)
    return
  }

  if (isPluginObject(entry)) {
    plugins.push(entry)
  }
}

async function loadPluginReference(reference, setupContext) {
  const specifier = reference.trim()
  if (!specifier) return null

  const root = path.resolve(setupContext.root ?? process.cwd())
  const resolved = resolvePluginReference(specifier, root)
  const mod = await import(pathToFileURL(resolved).href)
  const pluginExport = pluginExportFromModule(mod)

  if (pluginExport !== undefined && pluginExport !== null) {
    return pluginExport
  }

  throw new Error(
    `Plugin '${specifier}' must export a default plugin, a 'plugin' export, or a 'plugins' export.`,
  )
}

function pluginExportFromModule(mod) {
  const candidates = [
    mod.default,
    mod.plugin,
    mod.plugins,
    mod.default?.plugin,
    mod.default?.plugins,
    mod,
  ]

  return candidates.find(isPluginExport)
}

function isPluginExport(value) {
  return Array.isArray(value) || typeof value === "function" || isPluginObject(value)
}

function resolvePluginReference(specifier, root) {
  const requireFromRoot = createRequire(path.join(root, "ruvyxa.config.mjs"))
  const candidates = pluginSpecifiers(specifier)
  const errors = []

  for (const candidate of candidates) {
    try {
      return requireFromRoot.resolve(candidate)
    } catch (error) {
      errors.push(error)
    }
  }

  const message = errors.find((error) => error?.message)?.message ?? "module not found"
  throw new Error(`Cannot load plugin '${specifier}'. Install it or check the package name. ${message}`)
}

function pluginSpecifiers(specifier) {
  const candidates = [specifier]

  if (!isPathLikeSpecifier(specifier) && !specifier.startsWith("@")) {
    candidates.push(`ruvyxa-plugin-${specifier}`)
    candidates.push(`@ruvyxa/plugin-${specifier}`)
  }

  return candidates
}

function isPathLikeSpecifier(specifier) {
  return specifier.startsWith(".") || specifier.startsWith("/") || path.isAbsolute(specifier)
}

function isPluginObject(value) {
  return value && typeof value === "object" && typeof value.name === "string" && value.name.trim() !== ""
}

function pluginTimeoutMs(plugin) {
  return Number.isFinite(plugin.timeoutMs) && plugin.timeoutMs > 0
    ? Math.floor(plugin.timeoutMs)
    : DEFAULT_PLUGIN_TIMEOUT_MS
}

function withTimeout(promise, timeoutMs, onTimeout) {
  let timer
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => {
      try {
        onTimeout()
      } catch (error) {
        reject(error)
      }
    }, timeoutMs)
  })

  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer))
}

function stringValue(value) {
  return typeof value === "string" ? value : undefined
}
