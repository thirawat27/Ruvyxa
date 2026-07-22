import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  stat,
  writeFile,
} from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { after, describe, it } from 'node:test'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  clearCompilerCache,
  compilerCacheStats,
  compileBundle,
  compileBundleWithMetadata,
  invalidateCompilerCache,
  toImportPath,
} from '../../../packages/ruvyxa/runtime/compiler.mjs'
import { createFixtureWorkspace } from './fixture-workspace.mjs'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const exampleRoot = path.join(workspaceRoot, 'examples/demo')
const configRenderer = path.join(workspaceRoot, 'packages/ruvyxa/runtime/config-renderer.mjs')
const pluginRuntime = path.join(workspaceRoot, 'packages/ruvyxa/runtime/plugin-runtime.mjs')
const fixtureWorkspace = await createFixtureWorkspace('ruvyxa-compiler-tests-', exampleRoot)
after(() => rm(fixtureWorkspace, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }))

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

  it('keeps nested YAML, GFM, footnotes, and heading slugs aligned in the Node compiler', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'parity.mdx')
      const outfile = path.join(outDir, 'content-parity.mjs')
      await writeFile(
        pageFile,
        `---
title: "Ruvyxa: Content"
author:
  name: Ada
tags:
  - rust
  - mdx
summary: |
  First line.
  Second line.
---
# Repeat
# Repeat
## ภาษาไทย
## 🚀
## ✨

| Left | Right |
| :--- | ----: |
| one | two |

- [x] ~~shipped~~

A note[^1]

[^1]: Footnote body.
`,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { default, frontmatter, headings } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:content-parity-entry.ts',
        outfile,
        platform: 'node',
        external: ['react', 'react/jsx-runtime'],
      })

      const output = await readFile(outfile, 'utf8')
      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.deepEqual(mod.frontmatter, {
        title: 'Ruvyxa: Content',
        author: { name: 'Ada' },
        tags: ['rust', 'mdx'],
        summary: 'First line.\nSecond line.\n',
      })
      assert.deepEqual(mod.headings, [
        { depth: 1, slug: 'repeat', text: 'Repeat' },
        { depth: 1, slug: 'repeat-1', text: 'Repeat' },
        { depth: 2, slug: 'ภาษาไทย', text: 'ภาษาไทย' },
        { depth: 2, slug: 'section', text: '🚀' },
        { depth: 2, slug: 'section-1', text: '✨' },
      ])
      assert.match(output, /id:\s*"repeat-1"/)
      assert.match(output, /contains-task-list/)
      assert.match(output, /task-list-item/)
      assert.match(output, /data-footnotes/)
      assert.match(output, /textAlign/)
    })
  })

  it('preserves MDX metadata exported through aliases, functions, and classes', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'metadata-exports.mdx')
      const outfile = path.join(outDir, 'metadata-exports.mjs')
      await writeFile(
        pageFile,
        `export const customHeadings = [{ depth: 1, slug: 'custom', text: 'Custom' }]
export { customHeadings as headings }
export function meta() { return 'custom-meta' }
export async function frontmatter() { return {} }
export class contentFormat {}

# Generated heading
`,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { headings, meta, frontmatter, contentFormat } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:metadata-exports-entry.ts',
        outfile,
        platform: 'node',
        external: ['react', 'react/jsx-runtime'],
      })

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.deepEqual(mod.headings, [{ depth: 1, slug: 'custom', text: 'Custom' }])
      assert.equal(mod.meta(), 'custom-meta')
      assert.equal(typeof mod.frontmatter, 'function')
      assert.equal(typeof mod.contentFormat, 'function')
    })
  })

  it('rejects invalid and non-mapping YAML frontmatter in the Node compiler', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'invalid.md')
      const outfile = path.join(outDir, 'invalid-content.mjs')
      const compile = () =>
        compileBundle({
          projectRoot: root,
          entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
          sourcefile: 'ruvyxa:invalid-content-entry.ts',
          outfile,
          platform: 'node',
          external: ['react', 'react/jsx-runtime'],
        })

      await writeFile(pageFile, '---\nauthor: [broken\n---\n# Page\n')
      await assert.rejects(compile(), /RUV1312 .*invalid YAML frontmatter/)

      clearCompilerCache()
      await writeFile(pageFile, '---\nhello\n---\n# Page\n')
      await assert.rejects(compile(), /RUV1312 .*frontmatter must be a YAML mapping/)

      clearCompilerCache()
      await writeFile(pageFile, '---\nvalue: .inf\n---\n# Page\n')
      await assert.rejects(compile(), /RUV1312 .*JSON-compatible values/)

      clearCompilerCache()
      await writeFile(pageFile, '---\n1: numeric key\n---\n# Page\n')
      await assert.rejects(compile(), /RUV1312 .*YAML mapping keys must be strings/)
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

  it('rewrites external CommonJS requires to ESM imports', async () => {
    await withFixture(async ({ root, outDir }) => {
      const entryFile = path.join(root, 'entry.cjs')
      const outfile = path.join(outDir, 'commonjs-external.mjs')
      await writeFile(
        entryFile,
        `const util = require('node:util')\nmodule.exports = { encoder: util.TextEncoder.name }\n`,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { encoder } from ${JSON.stringify(toImportPath(entryFile))}`,
        sourcefile: 'ruvyxa:commonjs-external-entry.js',
        outfile,
        platform: 'node',
      })

      const output = await readFile(outfile, 'utf8')
      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(mod.encoder, 'TextEncoder')
      assert.doesNotMatch(output, /require\(['"]node:util['"]\)/)
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
      assert.equal(stats.transforms, stats.maxEntries)
      assert.ok(stats.sources <= stats.maxEntries)
      assert.ok(stats.content <= stats.maxEntries)
      clearCompilerCache()
    })
  })

  it('reuses transformed modules across bundles with the same inputs', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      await writeFile(
        pageFile,
        'export default function Page() { return <main>cached transform</main> }\n',
      )
      clearCompilerCache()
      const input = {
        projectRoot: root,
        entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:transform-cache-entry.ts',
        platform: 'node',
      }

      await compileBundle({ ...input, outfile: path.join(outDir, 'first.mjs') })
      const afterFirst = compilerCacheStats()
      await compileBundle({ ...input, outfile: path.join(outDir, 'second.mjs') })
      const afterSecond = compilerCacheStats()

      assert.ok(afterFirst.transforms > 0)
      assert.equal(afterSecond.transforms, afterFirst.transforms)
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
        jsxRuntime: 'classic',
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /React\.Fragment/)
      assert.match(output, /Object\.assign/)
      assert.doesNotMatch(output, /ignored/)
    })
  })

  it('uses the automatic JSX runtime by default and keeps classic mode opt-in', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const automaticOutfile = path.join(outDir, 'automatic-jsx.mjs')
      const classicOutfile = path.join(outDir, 'classic-jsx.mjs')
      await writeFile(pageFile, `export default function Page() { return <main>ready</main> }`)

      const input = {
        projectRoot: root,
        entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:jsx-runtime-entry.tsx',
        platform: 'browser',
        external: ['react', 'react/jsx-runtime'],
      }
      await compileBundle({ ...input, outfile: automaticOutfile })
      await compileBundle({ ...input, outfile: classicOutfile, jsxRuntime: 'classic' })

      const automatic = await readFile(automaticOutfile, 'utf8')
      const classic = await readFile(classicOutfile, 'utf8')
      assert.match(automatic, /jsx/)
      assert.doesNotMatch(automatic, /React\.createElement/)
      assert.match(classic, /React\.createElement/)
    })
  })

  it('uses a unique diagnostic code for invalid JSX runtime configuration', async () => {
    await withFixture(async ({ root, outDir }) => {
      await assert.rejects(
        compileBundle({
          projectRoot: root,
          entrySource: 'export default function Page() { return null }',
          sourcefile: 'ruvyxa:invalid-jsx-runtime.tsx',
          outfile: path.join(outDir, 'invalid-jsx-runtime.mjs'),
          platform: 'browser',
          jsxRuntime: 'invalid',
        }),
        /RUV1804 JSX runtime must be `classic` or `automatic`, got `invalid`/,
      )
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
        jsxRuntime: 'classic',
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
        jsxRuntime: 'classic',
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
        jsxRuntime: 'classic',
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /process\.env\.DATABASE_URL/)
      assert.doesNotMatch(output, /__exports\.POST/)
      assert.doesNotMatch(output, /__exports\.createTodo/)
    })
  })

  it('rejects private environment reads inside template expressions in client bundles', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      await writeFile(pageFile, 'export default `${process.env.DATABASE_URL}`\n')

      await assert.rejects(
        compileBundle({
          projectRoot: root,
          entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
          sourcefile: 'ruvyxa:template-env-entry.ts',
          outfile: path.join(outDir, 'template-env.mjs'),
          platform: 'browser',
        }),
        /RUV1008: Private environment variable DATABASE_URL used in client bundle/,
      )
    })
  })

  it('rejects private environment reads that follow a regular expression literal', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      // A quote inside the character class used to open a string that ran to
      // end-of-file, so the env read below was never seen and the secret
      // shipped to the browser without a diagnostic.
      await writeFile(
        pageFile,
        'const quoted = /[\'"]/g\nexport default () => quoted.test(process.env.DATABASE_URL)\n',
      )

      await assert.rejects(
        compileBundle({
          projectRoot: root,
          entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
          sourcefile: 'ruvyxa:regex-env-entry.ts',
          outfile: path.join(outDir, 'regex-env.mjs'),
          platform: 'browser',
        }),
        /RUV1008: Private environment variable DATABASE_URL used in client bundle/,
      )
    })
  })

  it('treats division as division when checking the client boundary', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      await writeFile(
        pageFile,
        'export const ratio = (a: number, b: number) => a / b / 2\nexport default () => ratio(1, 2)\n',
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:division-entry.ts',
        outfile: path.join(outDir, 'division.mjs'),
        platform: 'browser',
      })
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
        jsxRuntime: 'classic',
      })

      const output = await readFile(outfile, 'utf8')
      assert.doesNotMatch(output, /import "\.\/global\.css"/)
    })
  })

  it('exports deterministic class maps for CSS and SCSS modules', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      const outfile = path.join(outDir, 'style-modules.mjs')
      await writeFile(
        path.join(root, 'card.module.css'),
        `.base { color: navy; }
.card {
  composes: base;
  & .title { color: white; }
  :global(.theme-dark) .icon { color: black; }
}
`,
      )
      await writeFile(path.join(root, '_tokens.scss'), '$accent: rebeccapurple;\n')
      await writeFile(
        path.join(root, 'panel.module.scss'),
        "@use './tokens' as t; .panel { color: t.$accent; }\n",
      )
      await writeFile(
        pageFile,
        `
          import card from './card.module.css'
          import panel from './panel.module.scss'
          export const classes = [card.card, card.base, card.title, card.icon, card['theme-dark'], panel.panel]
        `,
      )

      const result = await compileBundleWithMetadata({
        projectRoot: root,
        entrySource: `export { classes } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:style-module-entry.ts',
        outfile,
        platform: 'node',
      })
      const mod = await import(`${pathToFileURL(outfile).href}?t=${Date.now()}`)

      assert.deepEqual(mod.classes[0].split(' '), ['card_card__a0c386682b31a0c2', mod.classes[1]])
      assert.match(mod.classes[2], /^card_title__/)
      assert.match(mod.classes[3], /^card_icon__/)
      assert.equal(mod.classes[4], undefined)
      assert.equal(mod.classes[5], 'panel_panel__9ffbc1bad8f2e789')
      assert.ok(result.inputs.includes('card.module.css'))
      assert.ok(result.inputs.includes('panel.module.scss'))
      assert.ok(result.inputs.includes('_tokens.scss'))
    })
  })

  it('reports stable Sass diagnostics for invalid modules', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'invalid-style.ts')
      const outfile = path.join(outDir, 'invalid-style.mjs')
      await writeFile(path.join(root, 'broken.module.scss'), '.broken { color: $missing; }\n')
      await writeFile(
        pageFile,
        "import broken from './broken.module.scss'; export default broken\n",
      )

      await assert.rejects(
        compileBundle({
          projectRoot: root,
          entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
          sourcefile: 'ruvyxa:invalid-style-entry.ts',
          outfile,
          platform: 'node',
        }),
        /RUV1402 Sass compilation failed/,
      )
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
        jsxRuntime: 'classic',
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /style:\s*\{\s*color:\s*accent\s*\}/)
      assert.match(output, /React\.createElement\("style"/)
      assert.match(output, /\.card \{ color:/)
    })
  })

  it('loads TypeScript plugin metadata and executes registered transform hooks', async () => {
    await withFixture(async ({ root }) => {
      const pageFile = path.join(root, 'page.tsx')
      await writeFile(pageFile, 'export const label = "Original"\n')
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          import { config, definePlugin } from "ruvyxa/config"

          export default config({
            css: { entries: ["styles/global.css"] },
            plugins: [
              definePlugin({
                name: "replace-label",
                setup({ transform }) {
                  transform((code, id, ctx) => {
                    if (ctx.environment !== "client" || !id.endsWith("page.tsx")) return null
                    return { code: code.replace("Original", "Transformed") }
                  })
                },
              }),
            ],
          })
        `,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.equal(config.ok, true)
      assert.deepEqual(config.config.css.entries, ['styles/global.css'])
      assert.equal(config.config.plugins[0].name, 'replace-label')

      const transformed = await runJson(pluginRuntime, [root, 'transform'], {
        code: await readFile(pageFile, 'utf8'),
        id: pageFile,
        environment: 'client',
      })

      assert.equal(transformed.ok, true)
      assert.match(transformed.result.code, /Transformed/)
    })
  })

  it('runs Fetch-native middleware and build-complete hooks from one plugin registry', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          import { writeFile } from "node:fs/promises"
          import { definePlugin } from "ruvyxa/config"

          export default {
            plugins: [definePlugin({
              name: "native-hooks",
              setup({ addMiddleware, onBuildComplete }) {
                addMiddleware({
                  routes: ["/api/*"],
                  onRequest(request, { plugin }) {
                    const headers = new Headers(request.headers)
                    headers.set("x-plugin", plugin)
                    return new Request(request, { headers })
                  },
                  onResponse(_request, response) {
                    const headers = new Headers(response.headers)
                    headers.set("x-after", "yes")
                    return new Response(response.body, { status: response.status, headers })
                  },
                })
                onBuildComplete(({ outDir, manifest }) =>
                  writeFile(outDir + "/plugin-complete.json", JSON.stringify(manifest))
                )
              },
            })],
          }
        `,
      )

      const described = await runJson(pluginRuntime, [root, 'describe'], {})
      assert.deepEqual(described.result, {
        plugins: ['native-hooks'],
        middleware: {
          request: 1,
          response: 1,
          requestRoutes: ['/api/*'],
          responseRoutes: ['/api/*'],
        },
        resolveId: 0,
        transform: 0,
        buildComplete: 1,
      })

      const request = await runJson(pluginRuntime, [root, 'middlewareRequest'], {
        request: { method: 'GET', path: '/api/users?active=1', headers: [] },
      })
      assert.equal(request.result.kind, 'request')
      assert.deepEqual(request.result.request.headers, [['x-plugin', 'native-hooks']])
      assert.equal(request.result.request.path, '/api/users?active=1')

      const response = await runJson(pluginRuntime, [root, 'middlewareResponse'], {
        request: request.result.request,
        response: {
          status: 200,
          headers: [
            ['content-type', 'application/octet-stream'],
            ['set-cookie', 'a=1; Path=/'],
            ['set-cookie', 'b=2; Path=/'],
          ],
          bodyBase64: Buffer.from([0, 255, 1]).toString('base64'),
        },
      })
      assert.equal(response.result.response.bodyBase64, Buffer.from([0, 255, 1]).toString('base64'))
      assert.equal(response.result.response.headers.find(([name]) => name === 'x-after')[1], 'yes')
      assert.deepEqual(
        response.result.response.headers.filter(([name]) => name === 'set-cookie'),
        [
          ['set-cookie', 'a=1; Path=/'],
          ['set-cookie', 'b=2; Path=/'],
        ],
      )

      const outDir = path.join(root, 'dist')
      await mkdir(outDir)
      const manifest = { routes: [{ path: '/' }] }
      const complete = await runJson(pluginRuntime, [root, 'buildComplete'], { outDir, manifest })
      assert.equal(complete.ok, true)
      assert.deepEqual(
        JSON.parse(await readFile(path.join(outDir, 'plugin-complete.json'), 'utf8')),
        manifest,
      )
    })
  })

  it('loads first-party plugins through the public ruvyxa/plugins entrypoint', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          import { contentEngine, observability, openApi } from "ruvyxa/plugins"

          export default {
            plugins: [
              observability({ routes: ["/api/*"], log: false }),
              contentEngine({
                siteUrl: "https://example.com",
                title: "Fixture content",
                description: "Fixture articles",
              }),
              openApi({
                info: { title: "Fixture API", version: "1.0.0" },
                operations: [{ method: "get", path: "/api/health" }],
              }),
            ],
          }
        `,
      )

      const described = await runJson(pluginRuntime, [root, 'describe'], {})
      assert.deepEqual(described.result, {
        plugins: ['ruvyxa:observability', 'ruvyxa:content-engine', 'ruvyxa:openapi'],
        middleware: {
          request: 3,
          response: 1,
          requestRoutes: [
            '/api/*',
            '/content.json',
            '/search-index.json',
            '/rss.xml',
            '/sitemap.xml',
            '/llms.txt',
            '/openapi.json',
          ],
          responseRoutes: ['/api/*'],
        },
        resolveId: 0,
        transform: 0,
        buildComplete: 2,
      })
      const configCache = path.join(root, '.ruvyxa', 'cache', 'config')
      const compiledConfigs = await Promise.all(
        (await readdir(configCache))
          .filter((name) => name.endsWith('.mjs'))
          .map((name) => readFile(path.join(configCache, name), 'utf8')),
      )
      assert.doesNotMatch(compiledConfigs.join('\n'), /^import \* as \w+ from ["']yaml["'];$/m)

      const requestResult = await runJson(pluginRuntime, [root, 'middlewareRequest'], {
        request: { method: 'GET', path: '/api/health', headers: [] },
      })
      assert.equal(requestResult.result.kind, 'request')
      assert.match(
        requestResult.result.request.headers.find(([name]) => name === 'x-request-id')[1],
        /^[0-9a-f-]{36}$/,
      )

      const specResult = await runJson(pluginRuntime, [root, 'middlewareRequest'], {
        request: { method: 'GET', path: '/openapi.json', headers: [] },
      })
      assert.equal(specResult.result.kind, 'response')
      assert.equal(
        JSON.parse(Buffer.from(specResult.result.response.bodyBase64, 'base64')).info.title,
        'Fixture API',
      )
    })
  })

  it('rejects middleware route patterns that can never match a pathname', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [{
            name: 'invalid-route',
            setup({ addMiddleware }) {
              addMiddleware({ routes: ['api/*'], onRequest() {} })
            },
          }],
        }`,
      )

      const failed = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(failed.exitCode, 1)
      assert.equal(failed.parsed.ok, false)
      assert.match(failed.parsed.message, /middleware routes\[0\].*start with "\/" or equal "\*"/)
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
        `export const plugin = { name: "label", setup({ transform }) { transform(code => code + "\\n// one") } }\n`,
      )

      const first = await runJson(configRenderer, [root], {})
      await writeFile(
        pluginFile,
        `export const plugin = { name: "label", setup({ transform }) { transform(code => code + "\\n// two") } }\n`,
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

  it('forwards render and middleware configuration to the Ruvyxa CLI', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          build: { prerenderCache: false },
          render: { strategy: 'isr', revalidate: 90 },
          middleware: {
            workers: 2,
            timeoutMs: 15000,
            builtin: { timing: false, headers: { 'X-Frame-Options': 'DENY' } }
          }
        }`,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.deepEqual(config.config.render, {
        strategy: 'isr',
        revalidate: 90,
      })
      assert.deepEqual(config.config.build, { prerenderCache: false })
      assert.deepEqual(config.config.middleware, {
        workers: 2,
        timeoutMs: 15000,
        builtin: { timing: false, headers: { 'X-Frame-Options': 'DENY' } },
      })
    })
  })

  it('serializes the selected JavaScript runtime for the CLI', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(path.join(root, 'ruvyxa.config.ts'), `export default { runtime: 'bun' }`)

      const config = await runJson(configRenderer, [root], {})
      assert.equal(config.config.runtime, 'bun')
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
  const root = await mkdtemp(path.join(fixtureWorkspace, 'fixture-'))
  const outDir = path.join(root, '.ruvyxa', 'cache')
  await mkdir(outDir, { recursive: true })

  try {
    await run({ root, outDir })
  } finally {
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  }
}
