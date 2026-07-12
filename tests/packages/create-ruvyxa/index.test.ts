import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, relative } from 'node:path'

import { createRuvyxaApp } from '../../../packages/create-ruvyxa/dist/index.js'

describe('createRuvyxaApp', () => {
  it('creates the minimal Next-style starter shape', async () => {
    const tempRoot = await mkdtemp(join(tmpdir(), 'ruvyxa-create-'))
    const target = join(tempRoot, 'my-app')

    try {
      await createRuvyxaApp(target)

      assert.deepEqual(await listFiles(target), [
        '.gitignore',
        'AGENTS.md',
        'CLAUDE.md',
        'app/css.d.ts',
        'app/globals.css',
        'app/layout.tsx',
        'app/page.tsx',
        'package.json',
        'public/ruvyxa.png',
        'ruvyxa.config.ts',
        'tsconfig.json',
      ])
      const packageJson = await readPackageJson(target)
      assert.equal(packageJson.name, 'my-app')
      assert.equal(packageJson.scripts.build, 'ruvyxa build')
    } finally {
      await rm(tempRoot, { recursive: true, force: true })
    }
  })

  it('derives a portable package name from the selected project directory', async () => {
    const tempRoot = await mkdtemp(join(tmpdir(), 'ruvyxa-create-'))
    const target = join(tempRoot, 'Big App_v2')

    try {
      await createRuvyxaApp(target)
      assert.equal((await readPackageJson(target)).name, 'big-app_v2')
    } finally {
      await rm(tempRoot, { recursive: true, force: true })
    }
  })

  it('rejects Windows reserved project names', async () => {
    await assert.rejects(createRuvyxaApp('CON'), /reserved or unsafe/)
    await assert.rejects(createRuvyxaApp('lpt1.txt'), /reserved or unsafe/)
  })

  it('rejects project names ending with unsafe Windows characters', async () => {
    await assert.rejects(createRuvyxaApp('my-app.'), /reserved or unsafe/)
    await assert.rejects(createRuvyxaApp('my-app '), /whitespace/)
  })
})

async function listFiles(root: string): Promise<string[]> {
  const files: string[] = []
  await visit(root)
  return files.sort()

  async function visit(dir: string) {
    const entries = await readdir(dir, { withFileTypes: true })
    for (const entry of entries) {
      const path = join(dir, entry.name)
      if (entry.isDirectory()) {
        await visit(path)
      } else {
        files.push(relative(root, path).replaceAll('\\', '/'))
      }
    }
  }
}

async function readPackageJson(root: string): Promise<{
  name: string
  scripts: Record<string, string>
}> {
  return JSON.parse(await readFile(join(root, 'package.json'), 'utf8'))
}
