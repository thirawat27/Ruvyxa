import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, stat, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  clearCompilerCache,
  compilerCacheStats,
  compileBundle,
  invalidateCompilerCache,
  toImportPath,
} from '../../../packages/ruvyxa/runtime/compiler.mjs'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const exampleRoot = path.join(workspaceRoot, 'examples/demo')
const configRenderer = path.join(workspaceRoot, 'packages/ruvyxa/runtime/config-renderer.mjs')
const pluginRunner = path.join(workspaceRoot, 'packages/ruvyxa/runtime/plugin-runner.mjs')

describe('runtime compiler', () => {
  it('resolves runtime aliases when the runtime path contains spaces', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa runtime path '))
    try {
      const packageRoot = path.join(root, 'package')
      const runtimeDir = path.join(packageRoot, 'runtime')
      const sourceDir = path.join(packageRoot, 'src')
      await mkdir(runtimeDir, { recursive: true })
      await mkdir(sourceDir, { recursive: true })
      await copyFile(
        path.join(workspaceRoot, 'packages/ruvyxa/runtime/compiler.mjs'),
        path.join(runtimeDir, 'compiler.mjs'),
      )
      await writeFile(path.join(sourceDir, 'index.ts'), 'export {}\n')

      const copiedCompiler = await import(
        `${pathToFileURL(path.join(runtimeDir, 'compiler.mjs')).href}?t=${Date.now()}`
      )
      const aliases = copiedCompiler.runtimeAliases()

      assert.equal(await realpath(aliases.ruvyxa), await realpath(path.join(sourceDir, 'index.ts')))
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('compiles Markdown and MDX modules with frontmatter and components', async () => {
    await withFixture(async ({ root, outDir }) => {
      const cardFile = path.join(root, 'Card.js')
      const pageFile = path.join(root, 'page.mdx')
      const outfile = path.join(outDir, 'content.mjs')
      await writeFile(cardFile, 'export default function Card({ children }) { return children }\n')
      await writeFile(
        pageFile,
        `---
title: Built-in MDX
draft: false
---
import Card from './Card.js'

# Hello MDX

<Card>{2 + 2}</Card>
`,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { default, frontmatter, headings } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:content-entry.ts',
        outfile,
        platform: 'node',
        external: ['react', 'react/jsx-runtime'],
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /Built-in MDX/)
      assert.match(output, /Hello MDX/)
      assert.match(output, /frontmatter/)
      assert.match(output, /2 \+ 2/)
    })
  })

  it('resolves local dynamic imports without an external bundler', async () => {
    await withFixture(async ({ root, outDir }) => {
      await writeFile(path.join(root, 'lazy.ts'), 'export const value = 42\n')
      const outfile = path.join(outDir, 'dynamic.mjs')

      await compileBundle({
        projectRoot: root,
        entrySource: `
          export async function load() {
            const mod = await import("./lazy.js")
            return mod.value
          }
        `,
        sourcefile: 'ruvyxa:dynamic-entry.ts',
        outfile,
        platform: 'node',
      })

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(await mod.load(), 42)
    })
  })

  it('lowers TypeScript enums and namespaces through Oxc', async () => {
    await withFixture(async ({ root, outDir }) => {
      const moduleFile = path.join(root, 'typed.ts')
      const outfile = path.join(outDir, 'typed.mjs')
      await writeFile(
        moduleFile,
        `
          enum Mode { Development, Production = 4 }
          namespace BuildInfo { export const label: string = 'ready' }
          export const mode = Mode.Production
          export const label = BuildInfo.label
        `,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { mode, label } from ${JSON.stringify(toImportPath(moduleFile))}`,
        sourcefile: 'ruvyxa:typed-entry.ts',
        outfile,
        platform: 'node',
      })

      const output = await readFile(outfile, 'utf8')
      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.doesNotMatch(output, /\benum\s+Mode\b|\bnamespace\s+BuildInfo\b/)
      assert.equal(mod.mode, 4)
      assert.equal(mod.label, 'ready')
    })
  })

  it('keeps code valid when runtime compiler minification is requested', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      const outfile = path.join(outDir, 'minified.mjs')
      await writeFile(
        pageFile,
        `
          export const label = 'preserve  internal  whitespace'
          // This comment must not consume the following export.
          export const answer = 42
        `,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { label, answer } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:minify-entry.ts',
        outfile,
        platform: 'browser',
        minify: true,
      })

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(mod.label, 'preserve  internal  whitespace')
      assert.equal(mod.answer, 42)
    })
  })

  it('initializes shared dependencies before importers across client graph branches', async () => {
    await withFixture(async ({ root, outDir }) => {
      const reactFile = path.join(root, 'react.js')
      const rendererFile = path.join(root, 'renderer.js')
      const pageFile = path.join(root, 'page.js')
      const outfile = path.join(outDir, 'dependency-order.mjs')

      await writeFile(
        reactFile,
        `
          export function useState(value) { return [value] }
          export function useEffect() {}
        `,
      )
      await writeFile(
        rendererFile,
        `
          import { useState } from 'react'
          export function render(Page) { return Page(useState) }
        `,
      )
      await writeFile(
        pageFile,
        `
          'use client'
          import { useEffect, useState } from 'react'
          export default function Page(rendererHook) {
            return rendererHook === useState && typeof useEffect === 'function'
          }
        `,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `
          import { render } from ${JSON.stringify(toImportPath(rendererFile))}
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export const initialized = render(Page)
        `,
        sourcefile: 'ruvyxa:client-dependency-order-entry.tsx',
        outfile,
        platform: 'browser',
        aliases: { react: reactFile },
      })

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(mod.initialized, true)
    })
  })

  it('rejects circular local dependencies before emitting an invalid bundle', async () => {
    await withFixture(async ({ root, outDir }) => {
      const firstFile = path.join(root, 'first.js')
      const secondFile = path.join(root, 'second.js')
      const outfile = path.join(outDir, 'circular.mjs')

      await writeFile(
        firstFile,
        `import { second } from './second.js'\nexport const first = 'first:' + second\n`,
      )
      await writeFile(
        secondFile,
        `import { first } from './first.js'\nexport const second = 'second:' + first\n`,
      )

      await assert.rejects(
        compileBundle({
          projectRoot: root,
          entrySource: `export { first } from ${JSON.stringify(toImportPath(firstFile))}`,
          sourcefile: 'ruvyxa:circular-entry.js',
          outfile,
          platform: 'browser',
        }),
        /RUV1803 circular dependency detected: first\.js -> second\.js -> first\.js/,
      )
    })
  })

  it('rewrites executable CommonJS requires without changing literal examples', async () => {
    await withFixture(async ({ root, outDir }) => {
      const dependencyFile = path.join(root, 'dependency.cjs')
      const entryFile = path.join(root, 'entry.js')
      const outfile = path.join(outDir, 'commonjs-literals.mjs')
      const specifier = './dependency.cjs'

      await writeFile(dependencyFile, `module.exports = { value: 42 }\n`)
      await writeFile(
        entryFile,
        [
          `const dependency = require(${JSON.stringify(specifier)})`,
          `const example = ${JSON.stringify(`require(${JSON.stringify(specifier)})`)}`,
          `const template = \`require(${JSON.stringify(specifier)})\``,
          `// require(${JSON.stringify(specifier)}) must stay documentation`,
          `export const result = { value: dependency.value, example, template }`,
          '',
        ].join('\n'),
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { result } from ${JSON.stringify(toImportPath(entryFile))}`,
        sourcefile: 'ruvyxa:commonjs-literal-entry.js',
        outfile,
        platform: 'browser',
      })

      const output = await readFile(outfile, 'utf8')
      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.deepEqual(mod.result, {
        value: 42,
        example: `require(${JSON.stringify(specifier)})`,
        template: `require(${JSON.stringify(specifier)})`,
      })
      assert.match(output, /must stay documentation/)
    })
  })

  it('recompiles a changed source after compiler-cache invalidation', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      const outfile = path.join(outDir, 'cache-invalidation.mjs')
      const compile = () =>
        compileBundle({
          projectRoot: root,
          entrySource: `export { value } from ${JSON.stringify(toImportPath(pageFile))}`,
          sourcefile: 'ruvyxa:cache-invalidation-entry.ts',
          outfile,
          platform: 'node',
        })

      await writeFile(pageFile, `export const value = 'first'\n`)
      await compile()
      await writeFile(pageFile, `export const value = 'other'\n`)
      invalidateCompilerCache([pageFile])
      await compile()

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(mod.value, 'other')
      clearCompilerCache()
    })
  })

  it('bounds compiler derivation caches across many unique bundles', async () => {
    await withFixture(async ({ root, outDir }) => {
      const outfile = path.join(outDir, 'bounded-cache.mjs')
      clearCompilerCache()
      for (let index = 0; index < 513; index++) {
        await compileBundle({
          projectRoot: root,
          entrySource: `export const value = ${index}\n`,
          sourcefile: `ruvyxa:bounded-cache-${index}.ts`,
          outfile,
          platform: 'node',
        })
      }

      const stats = compilerCacheStats()
      assert.equal(stats.rewrites, stats.maxEntries)
      assert.ok(stats.sources <= stats.maxEntries)
      assert.ok(stats.content <= stats.maxEntries)
      clearCompilerCache()
    })
  })

  it('emits source maps and skips unchanged bundle writes', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      const outfile = path.join(outDir, 'mapped.mjs')
      await writeFile(pageFile, 'export const answer = 42\n')

      const input = {
        projectRoot: root,
        entrySource: `export * from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:mapped-entry.ts',
        outfile,
        platform: 'node',
      }

      await compileBundle(input)
      const before = await stat(outfile)
      const map = JSON.parse(await readFile(`${outfile}.map`, 'utf8'))
      assert.equal(map.version, 3)
      assert.equal(map.file, path.basename(outfile))
      assert.ok(map.sources.some((source) => source.endsWith('/page.ts')))
      assert.ok(map.sourcesContent.some((source) => source.includes('answer = 42')))

      await new Promise((resolve) => setTimeout(resolve, 25))
      await compileBundle(input)
      const after = await stat(outfile)
      assert.equal(after.mtimeMs, before.mtimeMs)
    })
  })

  it('handles TSX fragments, spread props, and JSX comments', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const outfile = path.join(outDir, 'jsx.mjs')
      await writeFile(
        pageFile,
        `
          export default function Page(props) {
            return <><main {...props} className="shell">{/* ignored */}<span>{"ok"}</span></main></>
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: 'ruvyxa:jsx-entry.tsx',
        outfile,
        platform: 'browser',
        external: ['react'],
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /React\.Fragment/)
      assert.match(output, /Object\.assign/)
      assert.doesNotMatch(output, /ignored/)
    })
  })

  it('rewrites named class exports before wrapping modules', async () => {
    await withFixture(async ({ root, outDir }) => {
      const classFile = path.join(root, 'boundary.js')
      const outfile = path.join(outDir, 'class-export.mjs')
      await writeFile(classFile, `export class Boundary {\n  message() { return 'ready' }\n}\n`)

      await compileBundle({
        projectRoot: root,
        entrySource: `export { Boundary } from ${JSON.stringify(toImportPath(classFile))}`,
        sourcefile: 'ruvyxa:class-export-entry.js',
        outfile,
        platform: 'browser',
      })

      const output = await readFile(outfile, 'utf8')
      assert.doesNotMatch(output, /export class Boundary/)
      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(new mod.Boundary().message(), 'ready')
    })
  })

  it('handles JSX returned from ternaries and map callbacks', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const outfile = path.join(outDir, 'jsx-expressions.mjs')
      await writeFile(
        pageFile,
        `
          export default function Page({ items = ["one"], active = true }) {
            return (
              <main>
                {active ? <strong>Active</strong> : <span>Idle</span>}
                <ul>{items.map((item) => <li key={item}>{item}</li>)}</ul>
              </main>
            )
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: 'ruvyxa:jsx-expression-entry.tsx',
        outfile,
        platform: 'browser',
        external: ['react'],
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /React\.createElement\("strong"/)
      assert.match(output, /items\.map\(\(item\) => React\.createElement\("li"/)
      assert.doesNotMatch(output, /=> <li/)
    })
  })

  it('handles fragments in ternaries and dotted paths in code elements', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const outfile = path.join(outDir, 'jsx-edge-cases.mjs')
      await writeFile(
        pageFile,
        `
          export default function Page({ ready = true }) {
            return (
              <main>
                {ready ? <><span>Ready</span></> : <><span>Waiting</span></>}
                <code>.ruvyxa/prerender/static-page/index.html</code>
              </main>
            )
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from 'react'
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: 'ruvyxa:jsx-edge-cases-entry.tsx',
        outfile,
        platform: 'browser',
        external: ['react'],
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /React\.createElement\(React\.Fragment/)
      assert.match(output, /\.ruvyxa\/prerender\/static-page\/index\.html/)
      assert.doesNotMatch(output, /\? <>/)
    })
  })

  it('ignores import, export, and private env examples inside strings', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const outfile = path.join(outDir, 'string-examples.mjs')
      await writeFile(
        pageFile,
        `
          const snippet = \`
            import secret from "./missing"
            export function POST() {}
            export const createTodo = action
            process.env.DATABASE_URL
          \`

          export default function Page() {
            return <main>{snippet}</main>
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: 'ruvyxa:string-example-entry.tsx',
        outfile,
        platform: 'browser',
        external: ['react'],
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /process\.env\.DATABASE_URL/)
      assert.doesNotMatch(output, /__exports\.POST/)
      assert.doesNotMatch(output, /__exports\.createTodo/)
    })
  })

  it('drops side-effect asset imports from wrapped modules', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const outfile = path.join(outDir, 'asset-import.mjs')
      await writeFile(path.join(root, 'global.css'), 'body { margin: 0; }\n')
      await writeFile(
        pageFile,
        `
          import "./global.css"

          export default function Page() {
            return <main>ok</main>
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: 'ruvyxa:asset-import-entry.tsx',
        outfile,
        platform: 'browser',
        external: ['react'],
      })

      const output = await readFile(outfile, 'utf8')
      assert.doesNotMatch(output, /import "\.\/global\.css"/)
    })
  })

  it('preserves runtime CSS-in-JS style objects and style elements', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const outfile = path.join(outDir, 'css-in-js.mjs')
      await writeFile(
        pageFile,
        `
          const accent = "rebeccapurple"
          export default function Page() {
            return <main style={{ color: accent }}><style>{\`.card { color: \${accent}; }\`}</style>ok</main>
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: 'ruvyxa:css-in-js-entry.tsx',
        outfile,
        platform: 'browser',
        external: ['react'],
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /style:\s*\{\s*color:\s*accent\s*\}/)
      assert.match(output, /React\.createElement\("style"/)
      assert.match(output, /\.card \{ color:/)
    })
  })

  it('loads config plugin metadata and executes transform hooks', async () => {
    await withFixture(async ({ root }) => {
      const pageFile = path.join(root, 'page.tsx')
      await writeFile(pageFile, 'export const label = "Original"\n')
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          import { config } from "ruvyxa/config"

          export default config({
            css: { entries: ["styles/global.css"] },
            plugins: [
              {
                name: "replace-label",
                transform(code, id, ctx) {
                  if (ctx.environment !== "client" || !id.endsWith("page.tsx")) return null
                  return { code: code.replace("Original", "Transformed") }
                },
              },
            ],
          })
        `,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.equal(config.ok, true)
      assert.deepEqual(config.config.css.entries, ['styles/global.css'])
      assert.equal(config.config.plugins[0].name, 'replace-label')
      assert.equal(config.config.plugins[0].transform, true)

      const transformed = await runJson(pluginRunner, [root, 'transform'], {
        code: await readFile(pageFile, 'utf8'),
        id: pageFile,
        environment: 'client',
      })

      assert.equal(transformed.ok, true)
      assert.match(transformed.result.code, /Transformed/)
    })
  })

  it('changes the config dependency fingerprint when imported plugin code changes', async () => {
    await withFixture(async ({ root }) => {
      const pluginFile = path.join(root, 'plugin.ts')
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          import { plugin } from "./plugin.js"
          export default { plugins: [plugin] }
        `,
      )
      await writeFile(
        pluginFile,
        `export const plugin = { name: "label", transform(code) { return code + "\\n// one" } }\n`,
      )

      const first = await runJson(configRenderer, [root], {})
      await writeFile(
        pluginFile,
        `export const plugin = { name: "label", transform(code) { return code + "\\n// two" } }\n`,
      )
      const second = await runJson(configRenderer, [root], {})

      assert.match(first.dependencyHash, /^[a-f0-9]{64}$/)
      assert.match(second.dependencyHash, /^[a-f0-9]{64}$/)
      assert.notEqual(second.dependencyHash, first.dependencyHash)
    })
  })

  it('returns JSON for missing and failing config files', async () => {
    await withFixture(async ({ root }) => {
      const missing = await runJson(configRenderer, [root], {})
      assert.equal(missing.ok, true)
      assert.deepEqual(missing.config, {})
      assert.equal(missing.dependencyHash, 'no-config')

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          throw new Error("bad config")
          export default {}
        `,
      )

      const failed = await runJsonResult(configRenderer, [root], {})
      assert.equal(failed.exitCode, 1)
      assert.equal(failed.parsed.ok, false)
      assert.match(failed.parsed.message, /bad config/)
    })
  })

  it('serializes WebP image encoding controls', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { image: { optimize: true, quality: 91, lossless: true, workers: 2 } }`,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.deepEqual(config.config.image, {
        optimize: true,
        quality: 91,
        lossless: true,
        workers: 2,
      })
    })
  })

  it('serializes scalable action, API, and plugin security limits', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          security: {
            actionLimit: 2 * 1024 * 1024,
            apiLimit: 20 * 1024 * 1024,
            pluginLimit: 64 * 1024 * 1024,
            actionRateLimit: { max: 1200, window: 30 },
            trustedProxyIps: ['10.0.0.2', '2001:db8::1']
          }
        }`,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.deepEqual(config.config.security, {
        actionLimit: 2 * 1024 * 1024,
        apiLimit: 20 * 1024 * 1024,
        pluginLimit: 64 * 1024 * 1024,
        actionRateLimit: { max: 1200, window: 30 },
        trustedProxyIps: ['10.0.0.2', '2001:db8::1'],
      })
    })
  })

  it('forwards render and middleware configuration to the native CLI', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          render: { strategy: 'isr', revalidate: 90 },
          middleware: { builtin: { timing: false, headers: { 'X-Frame-Options': 'DENY' } } }
        }`,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.deepEqual(config.config.render, {
        strategy: 'isr',
        revalidate: 90,
      })
      assert.deepEqual(config.config.middleware, {
        builtin: { timing: false, headers: { 'X-Frame-Options': 'DENY' } },
      })
    })
  })

  it('executes adapters and serializes their deployment metadata', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          outDir: '.output',
          adapterOptions: { region: 'iad1' },
          adapter: {
            name: 'fixture',
            target: 'static',
            build({ root, outDir }) {
              return {
                name: 'fixture',
                target: 'static',
                platform: 'static',
                entry: outDir + '/static',
                assetsDir: outDir + '/assets',
                clientDir: outDir + '/client',
                root,
              }
            },
          },
        }`,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.deepEqual(config.config.adapter, {
        name: 'fixture',
        target: 'static',
        platform: 'static',
        entry: '.output/static',
        assetsDir: '.output/assets',
        clientDir: '.output/client',
        root,
      })
      assert.deepEqual(config.config.adapterOptions, { region: 'iad1' })
    })
  })

  it('rejects unknown config fields instead of silently ignoring them', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { debug: { overlay: true, tracez: true } }`,
      )

      const failed = await runJsonResult(configRenderer, [root], {})
      assert.equal(failed.exitCode, 1)
      assert.equal(failed.parsed.ok, false)
      assert.match(failed.parsed.message, /RUV1602 unknown config\.debug field: tracez/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { image: { formats: ["avif", "webp"] } }`,
      )
      const obsolete = await runJsonResult(configRenderer, [root], {})
      assert.equal(obsolete.exitCode, 1)
      assert.match(obsolete.parsed.message, /RUV1602 unknown config\.image field: formats/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { render: { fallback: 'blocking' } }`,
      )
      const removedRenderingOption = await runJsonResult(configRenderer, [root], {})
      assert.equal(removedRenderingOption.exitCode, 1)
      assert.match(
        removedRenderingOption.parsed.message,
        /RUV1602 unknown config\.render field: fallback/,
      )

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { build: { sourcemap: false } }`,
      )
      const legacyBuildKey = await runJsonResult(configRenderer, [root], {})
      assert.equal(legacyBuildKey.exitCode, 1)
      assert.match(legacyBuildKey.parsed.message, /RUV1602 unknown config\.build field: sourcemap/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { security: { actionRateLimit: { burst: 10 } } }`,
      )
      const invalidRateLimit = await runJsonResult(configRenderer, [root], {})
      assert.equal(invalidRateLimit.exitCode, 1)
      assert.match(
        invalidRateLimit.parsed.message,
        /RUV1602 unknown config\.security\.actionRateLimit field: burst/,
      )

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { middleware: { builtin: { logging: false } } }`,
      )
      const legacyMiddlewareKey = await runJsonResult(configRenderer, [root], {})
      assert.equal(legacyMiddlewareKey.exitCode, 1)
      assert.match(
        legacyMiddlewareKey.parsed.message,
        /RUV1602 unknown config\.middleware\.builtin field: logging/,
      )
    })
  })

  it('rejects config values whose scalar types do not match the schema', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { server: { port: '3000' } }`,
      )

      const invalidNumber = await runJsonResult(configRenderer, [root], {})
      assert.equal(invalidNumber.exitCode, 1)
      assert.match(invalidNumber.parsed.message, /RUV1602 config\.server\.port must be number/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { security: { trustedProxyIps: '127.0.0.1' } }`,
      )
      const invalidArray = await runJsonResult(configRenderer, [root], {})
      assert.equal(invalidArray.exitCode, 1)
      assert.match(
        invalidArray.parsed.message,
        /RUV1602 config\.security\.trustedProxyIps must be string\[\]/,
      )
    })
  })
})

function runJson(script, args, payload) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, ...args], {
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk) => {
      stdout += chunk
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk
    })
    child.on('error', reject)
    child.on('close', (code) => {
      try {
        const parsed = JSON.parse(stdout)
        if (code === 0 && parsed.ok) {
          resolve(parsed)
        } else {
          reject(new Error(`script failed (${code}): ${stdout || stderr}`))
        }
      } catch (error) {
        reject(
          new Error(
            `invalid JSON from script: ${error.message}; stdout=${stdout}; stderr=${stderr}`,
          ),
        )
      }
    })
    child.stdin.end(JSON.stringify(payload))
  })
}

function runJsonResult(script, args, payload) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, ...args], {
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk) => {
      stdout += chunk
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk
    })
    child.on('error', reject)
    child.on('close', (exitCode) => {
      try {
        resolve({ exitCode, parsed: JSON.parse(stdout), stderr })
      } catch (error) {
        reject(
          new Error(
            `invalid JSON from script: ${error.message}; stdout=${stdout}; stderr=${stderr}`,
          ),
        )
      }
    })
    child.stdin.end(JSON.stringify(payload))
  })
}

async function withFixture(run) {
  const root = await mkdtemp(path.join(exampleRoot, '.ruvyxa-compiler-test-'))
  const outDir = path.join(root, '.ruvyxa', 'cache')
  await mkdir(outDir, { recursive: true })

  try {
    await run({ root, outDir })
  } finally {
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  }
}
