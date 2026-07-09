import { createInterface } from "node:readline/promises"
import { existsSync, readFileSync } from "node:fs"
import path from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"

import { cacheFileName, compileBundle, runtimeAliases, toImportPath } from "./compiler.mjs"
import { normalizePlugins, PluginHookError, runPluginHook } from "./plugin-utils.mjs"

const [projectRootArg, hook] = process.argv.slice(2)

if (!projectRootArg || !hook) {
  fail("RUV1701", "Plugin runner requires project root and hook arguments.")
}

const projectRoot = path.resolve(projectRootArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))

try {
  const config = await loadConfig(projectRoot)
  const plugins = await normalizePlugins(config.plugins, {
    root: projectRoot,
    command: commandName(),
  })

  if (hook === "--persistent") {
    await runPersistent(plugins)
  } else {
    const payload = JSON.parse(await readStdin())
    ok(await runHook(plugins, hook, payload))
  }
} catch (error) {
  fail(
    error instanceof PluginHookError ? "RUV1703" : "RUV1700",
    error instanceof Error ? error.message : String(error),
    error?.stack,
  )
}

async function runPersistent(plugins) {
  const lines = createInterface({
    input: process.stdin,
    crlfDelay: Infinity,
  })

  for await (const line of lines) {
    if (!line.trim()) continue

    try {
      const request = JSON.parse(line)
      const result = await runHook(plugins, request.hook, request.payload ?? {})
      writeJsonLine({ ok: true, result })
    } catch (error) {
      writeJsonLine({
        ok: false,
        code: error instanceof PluginHookError ? "RUV1703" : "RUV1700",
        message: error instanceof Error ? error.message : String(error),
        stack: error?.stack,
      })
    }
  }
}

async function runHook(plugins, hookName, payload) {
  if (hookName === "resolveId") {
    return runResolveId(plugins, payload)
  }

  if (hookName === "transform") {
    return runTransform(plugins, payload)
  }

  throw new Error(`Unknown plugin hook: ${hookName}`)
}

async function loadConfig(root) {
  const configFile = findConfig(root)
  if (!configFile) return {}

  const moduleCode = `export { default } from ${JSON.stringify(toImportPath(configFile))}`
  const outfile = path.join(
    root,
    ".ruvyxa",
    "cache",
    "config",
    cacheFileName([moduleCode, configFile, "plugin-runner"], "mjs"),
  )

  await compileBundle({
    projectRoot: root,
    entrySource: moduleCode,
    sourcefile: "ruvyxa:plugin-config-entry.ts",
    outfile,
    platform: "node",
    aliases: runtimeAliases(runtimeDir),
  })

  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
  return mod.default ?? {}
}

function findConfig(root) {
  for (const fileName of ["ruvyxa.config.ts", "ruvyxa.config.mts", "ruvyxa.config.js", "ruvyxa.config.mjs"]) {
    const file = path.join(root, fileName)
    if (existsSync(file)) return file
  }
  return null
}

async function runResolveId(plugins, payload) {
  const ctx = {
    environment: payload.environment ?? "client",
    root: projectRoot,
    command: commandName(),
  }

  for (const plugin of plugins) {
    if (typeof plugin.resolveId !== "function") continue

    const result = await runPluginHook(plugin, "resolveId", () => {
      return plugin.resolveId(payload.id, payload.importer ?? undefined, ctx)
    })
    if (typeof result === "string") {
      return result
    }
  }
  return null
}

async function runTransform(plugins, payload) {
  let code = String(payload.code ?? "")
  let changed = false
  const ctx = {
    environment: payload.environment ?? "client",
    root: projectRoot,
    id: String(payload.id ?? ""),
    command: commandName(),
  }

  for (const plugin of plugins) {
    if (typeof plugin.transform !== "function") continue

    const result = await runPluginHook(plugin, "transform", () => {
      return plugin.transform(code, String(payload.id ?? ""), ctx)
    })
    if (!result) continue

    if (typeof result === "string") {
      code = result
      changed = true
      continue
    }

    if (typeof result === "object" && typeof result.code === "string") {
      code = result.code
      changed = true
    }
  }

  return changed ? { code } : null
}

async function readStdin() {
  return readFileSync(0, "utf8")
}

function ok(result) {
  process.stdout.write(JSON.stringify({ ok: true, result }))
  process.exit(0)
}

function fail(code, message, stack) {
  process.stdout.write(JSON.stringify({ ok: false, code, message, stack }))
  process.exit(1)
}

function writeJsonLine(value) {
  process.stdout.write(`${JSON.stringify(value)}\n`)
}

function commandName() {
  return process.env.RUVYXA_COMMAND || "unknown"
}
