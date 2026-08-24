/**
 * Run the project's own PostCSS plugin chain over one collected stylesheet.
 *
 * Ruvyxa's global CSS pipeline lives in Rust: it walks stylesheet imports,
 * compiles Sass, scopes CSS modules, and concatenates the result. PostCSS is the
 * one stage that cannot live there — the plugins are the application's own
 * JavaScript, resolved from the application's `node_modules`. This script is
 * that stage, invoked once per global stylesheet entry with the already-inlined
 * CSS.
 *
 * Contract with `crates/ruvyxa_dev_server/src/postcss.rs`:
 *
 *   argv[2]  path to a JSON request file
 *            `{ root, config, from, cssFile, mode }`
 *   stdout   exactly one JSON line
 *            `{ ok: true, css, dependencies }`
 *            `{ ok: false, code, message }`
 *   exit     0 on success, 1 on a reported failure
 *
 * stdin is closed by the caller, so the request travels by file rather than by
 * pipe: a collected stylesheet is far larger than an argument list may be.
 */

import { writeSync } from 'node:fs'
import { readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import path from 'node:path'
import { pathToFileURL } from 'node:url'

async function main() {
  const requestFile = process.argv[2]
  if (!requestFile) {
    fail('RUV1405', 'css-runner.mjs requires a request file path')
    return
  }

  const request = JSON.parse(await readFile(requestFile, 'utf8'))
  const root = path.resolve(request.root)
  const from = path.resolve(request.from)
  const css = await readFile(request.cssFile, 'utf8')

  const postcss = await loadPostcss(root, request.config)
  const plugins = await loadPlugins(root, request.config, request.mode)

  // An empty chain is not an error, but it is also not worth a PostCSS pass:
  // returning the input unchanged keeps a config that only registers plugins
  // conditionally from rewriting the stylesheet for no reason.
  if (plugins.length === 0) {
    succeed(css, [])
    return
  }

  let result
  try {
    result = await postcss(plugins).process(css, { from, to: from, map: false })
  } catch (error) {
    fail('RUV1406', describePostcssError(error, request.config))
    return
  }

  // Plugins that read other files (Tailwind's source scanning, `postcss-import`)
  // report them as messages. They become watch inputs so a dev edit to a
  // template that only changes class names still regenerates the stylesheet.
  const dependencies = []
  for (const message of result.messages ?? []) {
    if (message.type === 'dependency' && message.file) dependencies.push(message.file)
    else if (message.type === 'dir-dependency' && message.dir) dependencies.push(message.dir)
  }

  succeed(result.css, dependencies)
}

async function loadPostcss(root, configFile) {
  const require = createRequire(path.join(root, '__ruvyxa-postcss__.cjs'))
  let resolved
  try {
    resolved = require.resolve('postcss')
  } catch {
    fail(
      'RUV1405',
      `${configFile} registers PostCSS plugins, but \`postcss\` is not installed in ${root}. ` +
        'Install it with `npm install -D postcss`.',
    )
    return null
  }
  const loaded = await import(pathToFileURL(resolved).href)
  return loaded.default ?? loaded
}

/**
 * Load the project's PostCSS configuration and resolve it to plugin instances.
 *
 * Accepts every shape `postcss-load-config` accepts: a plugin array, a
 * `{ name: options }` object, or a function of the build context.
 */
async function loadPlugins(root, configFile, mode) {
  const configPath = path.resolve(configFile)
  let config = await readConfig(root, configPath)

  if (typeof config === 'function') {
    config = config({ env: mode, mode, cwd: root, file: configPath })
  }
  config = await config

  const declared = config?.plugins ?? config
  if (!declared) return []

  const require = createRequire(path.join(root, '__ruvyxa-postcss__.cjs'))
  const entries = Array.isArray(declared)
    ? declared.map((plugin) => [plugin, undefined])
    : Object.entries(declared)

  const plugins = []
  for (const [plugin, options] of entries) {
    if (options === false || options === null) continue
    if (typeof plugin !== 'string') {
      plugins.push(plugin)
      continue
    }
    plugins.push(await instantiatePlugin(require, plugin, options, configFile, root))
  }
  return plugins.filter(Boolean)
}

async function instantiatePlugin(require, name, options, configFile, root) {
  let resolved
  try {
    resolved = require.resolve(name)
  } catch {
    fail(
      'RUV1405',
      `${configFile} registers the PostCSS plugin \`${name}\`, but it could not be resolved from ${root}. ` +
        `Install it with \`npm install -D ${name}\`.`,
    )
    return null
  }

  const loaded = await import(pathToFileURL(resolved).href)
  const factory = loaded.default ?? loaded
  // A PostCSS plugin is either a factory or an already-built plugin object. Only
  // a factory is called, and only with options the config actually supplied.
  if (typeof factory === 'function') {
    return options === undefined || options === true ? factory() : factory(options)
  }
  return factory
}

async function readConfig(root, configPath) {
  const extension = path.extname(configPath).toLowerCase()

  if (extension === '.json' || path.basename(configPath) === '.postcssrc') {
    return JSON.parse(await readFile(configPath, 'utf8'))
  }

  try {
    const loaded = await import(pathToFileURL(configPath).href)
    return loaded.default ?? loaded
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    const hint =
      extension === '.ts' || extension === '.mts' || extension === '.cts'
        ? ' A TypeScript PostCSS config needs a Node release that can load it directly; rename it to `postcss.config.mjs` if that is not available.'
        : ''
    fail('RUV1405', `Failed to load ${configPath} from ${root}: ${detail}.${hint}`)
    return null
  }
}

function describePostcssError(error, configFile) {
  if (!(error instanceof Error)) return `${String(error)} (from ${configFile})`
  // A `CssSyntaxError` carries the offending file and position; a plugin throw
  // usually does not. Report whichever the error actually has.
  const position = error.line ? `:${error.line}` : ''
  const location = error.file ? `${error.file}${position}: ` : ''
  return `${location}${error.reason ?? error.message} (plugin chain from ${configFile})`
}

/**
 * Emit the one JSON line this run owes its caller, then leave.
 *
 * `process.exit()` does not drain a pending asynchronous stdout write, and
 * stdout here is a pipe read by `postcss.rs`. This payload is a whole compiled
 * stylesheet, so it is the one of these helpers most able to outgrow a pipe
 * buffer. Writing straight to fd 1 removes the race instead of narrowing it.
 * See the stdio-protocol rule in `AGENTS.md`.
 */
function respondAndExit(payload, code) {
  writeSync(1, `${JSON.stringify(payload)}\n`)
  process.exit(code)
}

function succeed(css, dependencies) {
  respondAndExit({ ok: true, css, dependencies }, 0)
}

function fail(code, message) {
  respondAndExit({ ok: false, code, message }, 1)
}

try {
  await main()
} catch (error) {
  fail('RUV1406', error instanceof Error ? (error.stack ?? error.message) : String(error))
}
