import { existsSync } from "node:fs"
import { Buffer } from "node:buffer"
import path from "node:path"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import { build } from "esbuild"

const [projectRootArg] = process.argv.slice(2)

if (!projectRootArg) {
  fail("RUV1601", "Config renderer requires a project root argument.")
}

const projectRoot = path.resolve(projectRootArg)
const runtimeDir = dirname(fileURLToPath(import.meta.url))
const packageRoot = path.resolve(runtimeDir, "..")

try {
  const configFile = findConfig(projectRoot)
  if (!configFile) {
    ok({})
  }

  const result = await build({
    entryPoints: [configFile],
    bundle: true,
    platform: "node",
    format: "esm",
    write: false,
    absWorkingDir: projectRoot,
    sourcemap: false,
    logLevel: "silent",
    plugins: [ruvyxaConfigFallbackPlugin()],
  })

  const code = result.outputFiles[0]?.text
  if (!code) {
    fail("RUV1602", "Config bundling completed without output.")
  }

  const dataUrl = `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
  const mod = await import(dataUrl)
  const config = mod.default ?? {}
  ok(sanitizeConfig(config))
} catch (error) {
  fail("RUV1600", error instanceof Error ? error.message : String(error), error?.stack)
}

function ruvyxaConfigFallbackPlugin() {
  return {
    name: "ruvyxa-config-fallback",
    setup(build) {
      build.onResolve({ filter: /^ruvyxa\/config$/ }, (args) => {
        if (args.resolveDir.includes(`${path.sep}node_modules${path.sep}`)) {
          return undefined
        }

        for (const file of [
          path.join(packageRoot, "dist", "config.js"),
          path.join(packageRoot, "src", "config.ts"),
        ]) {
          if (existsSync(file)) return { path: file }
        }

        return undefined
      })
    },
  }
}

function findConfig(root) {
  for (const fileName of ["ruvyxa.config.ts", "ruvyxa.config.mts", "ruvyxa.config.js", "ruvyxa.config.mjs"]) {
    const file = path.join(root, fileName)
    if (existsSync(file)) return file
  }
  return null
}

function sanitizeConfig(config) {
  return {
    appDir: stringValue(config.appDir),
    outDir: stringValue(config.outDir),
    runtime: stringValue(config.runtime),
    react: booleanValue(config.react),
    server: objectValue(config.server, {
      host: stringValue(config.server?.host),
      port: numberValue(config.server?.port),
    }),
    build: objectValue(config.build, {
      minify: booleanValue(config.build?.minify),
      sourcemap: booleanValue(config.build?.sourcemap),
      splitStrategy: stringValue(config.build?.splitStrategy),
      parallelism: numberValue(config.build?.parallelism),
    }),
    debug: objectValue(config.debug, {
      overlay: booleanValue(config.debug?.overlay),
      traces: booleanValue(config.debug?.traces),
    }),
    security: objectValue(config.security, {
      actionBodyLimitBytes: numberValue(config.security?.actionBodyLimitBytes),
      sameOriginActions: booleanValue(config.security?.sameOriginActions),
      fetchMetadataActions: booleanValue(config.security?.fetchMetadataActions),
      securityHeaders: booleanValue(config.security?.securityHeaders),
    }),
    cache: objectValue(config.cache, {
      routeManifest: booleanValue(config.cache?.routeManifest),
      css: booleanValue(config.cache?.css),
    }),
    adapter: objectValue(config.adapter, {
      name: stringValue(config.adapter?.name),
      target: stringValue(config.adapter?.target),
    }),
  }
}

function objectValue(source, value) {
  if (!source || typeof source !== "object") return undefined
  const filtered = Object.fromEntries(Object.entries(value).filter(([, item]) => item !== undefined))
  return Object.keys(filtered).length > 0 ? filtered : undefined
}

function stringValue(value) {
  return typeof value === "string" ? value : undefined
}

function numberValue(value) {
  return Number.isFinite(value) ? value : undefined
}

function booleanValue(value) {
  return typeof value === "boolean" ? value : undefined
}

function ok(config) {
  process.stdout.write(JSON.stringify({ ok: true, config }))
  process.exit(0)
}

function fail(code, message, stack) {
  process.stdout.write(JSON.stringify({ ok: false, code, message, stack }))
  process.exit(1)
}
