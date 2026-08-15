import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const require = createRequire(
  path.join(path.dirname(fileURLToPath(import.meta.url)), '__react-compiler.cjs'),
)
let toolchain

/**
 * Run the stable React Compiler before Ruvyxa's Oxc syntax transform.
 *
 * This is deliberately opt-in and configuration-free in the first release:
 * React 19 is Ruvyxa's peer contract and inference mode is the upstream
 * default. Babel configuration files are disabled so a project cannot make
 * the framework's server and client lanes use different hidden transforms.
 */
export function transformWithReactCompiler(source, filename) {
  if (!/\.[cm]?[jt]sx?$/.test(filename)) return null
  toolchain ??= loadToolchain()
  let result
  try {
    result = toolchain.transformSync(source, {
      ast: false,
      babelrc: false,
      code: true,
      compact: false,
      configFile: false,
      filename,
      parserOpts: {
        plugins: [
          filename.endsWith('x') ? 'jsx' : null,
          /\.[cm]?tsx?$/.test(filename) ? 'typescript' : null,
        ].filter(Boolean),
        sourceType: 'module',
      },
      plugins: [[toolchain.compiler, { compilationMode: 'infer', target: '19' }]],
      sourceMaps: true,
      sourceType: 'module',
    })
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new Error(`RUV1850 React Compiler failed for ${filename}: ${detail}`)
  }
  if (typeof result?.code !== 'string') {
    throw new Error(`RUV1850 React Compiler produced no code for ${filename}`)
  }
  return {
    code: result.code,
    map: result.map ? JSON.stringify(result.map) : undefined,
    rawMap: result.map ?? undefined,
  }
}

function loadToolchain() {
  try {
    const { transformSync } = require('@babel/core')
    const loaded = require('babel-plugin-react-compiler')
    return { transformSync, compiler: loaded.default ?? loaded }
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new Error(
      `RUV1850 reactCompiler is enabled but its production toolchain is unavailable: ${detail}`,
    )
  }
}
