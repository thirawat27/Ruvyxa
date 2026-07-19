import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, readdir, rm, utimes, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, relative } from 'node:path'

import {
  createRuvyxaApp,
  detectPackageManager,
} from '../../../packages/create-ruvyxa/dist/index.js'

describe('detectPackageManager', () => {
  it("recognizes Bun's text lockfile", async () => {
    const root = await mkdtemp(join(tmpdir(), 'ruvyxa-bun-lock-'))
    try {
      await writeFile(join(root, 'bun.lock'), '{}')
      assert.deepEqual(detectPackageManager(root, {}), {
        name: 'bun',
        install: 'bun install',
        dev: 'bun dev',
        exec: 'bunx',
        lockfile: 'bun.lock',
      })
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('uses the closest packageManager declaration when lockfiles conflict', async () => {
    const root = await mkdtemp(join(tmpdir(), 'ruvyxa-package-manager-'))
    try {
      await writeFile(join(root, 'package.json'), JSON.stringify({ packageManager: 'yarn@4.7.0' }))
      await writeFile(join(root, 'pnpm-lock.yaml'), 'lockfileVersion: 9')
      await writeFile(join(root, 'package-lock.json'), '{}')

      assert.equal(detectPackageManager(root, {}).name, 'yarn')
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('uses the newest local lockfile when no explicit manager is declared', async () => {
    const root = await mkdtemp(join(tmpdir(), 'ruvyxa-lockfile-recency-'))
    try {
      const pnpmLock = join(root, 'pnpm-lock.yaml')
      const npmLock = join(root, 'package-lock.json')
      await writeFile(pnpmLock, 'lockfileVersion: 9')
      await writeFile(npmLock, '{}')
      const now = new Date()
      await utimes(pnpmLock, new Date(now.getTime() - 10_000), new Date(now.getTime() - 10_000))
      await utimes(npmLock, now, now)

      assert.equal(detectPackageManager(root, {}).name, 'npm')
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('uses the nearest project instead of a parent workspace lockfile', async () => {
    const root = await mkdtemp(join(tmpdir(), 'ruvyxa-nested-package-manager-'))
    const app = join(root, 'apps', 'web')
    try {
      await mkdir(app, { recursive: true })
      await writeFile(join(root, 'pnpm-lock.yaml'), 'lockfileVersion: 9')
      await writeFile(join(app, 'package-lock.json'), '{}')

      assert.equal(detectPackageManager(app, {}).name, 'npm')
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('prefers the invoking package manager over stale project evidence', async () => {
    const root = await mkdtemp(join(tmpdir(), 'ruvyxa-invoking-package-manager-'))
    try {
      await writeFile(join(root, 'bun.lock'), '{}')
      assert.equal(
        detectPackageManager(root, { npm_config_user_agent: 'pnpm/10.0.0' }).name,
        'pnpm',
      )
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
})

describe('createRuvyxaApp', () => {
  it('creates the minimal file-system starter shape', async () => {
    const tempRoot = await mkdtemp(join(tmpdir(), 'ruvyxa-create-'))
    const target = join(tempRoot, 'my-app')

    try {
      await createRuvyxaApp(target)

      assert.deepEqual(await listFiles(target), [
        '.gitignore',
        'AGENTS.md',
        'CLAUDE.md',
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

  for (const [template, expectedFile] of [
    ['blog', 'app/blog/[slug]/page.tsx'],
    ['crud', 'app/tasks/action.ts'],
    ['api-backend', 'app/api/items/[id]/route.ts'],
  ] as const) {
    it(`creates the ${template} starter`, async () => {
      const tempRoot = await mkdtemp(join(tmpdir(), 'ruvyxa-create-'))
      const target = join(tempRoot, `${template}-app`)

      try {
        await createRuvyxaApp(target, { template })
        const files = await listFiles(target)
        assert.ok(files.includes(expectedFile))
        assert.ok(files.includes('.gitignore'))
        assert.equal((await readPackageJson(target)).name, `${template}-app`)
      } finally {
        await rm(tempRoot, { recursive: true, force: true })
      }
    })
  }

  it('rejects unknown starter templates before changing files', async () => {
    const tempRoot = await mkdtemp(join(tmpdir(), 'ruvyxa-create-'))
    const target = join(tempRoot, 'unknown-app')
    try {
      await assert.rejects(
        createRuvyxaApp(target, { template: 'unknown' as never }),
        /Choose one of: minimal, blog, crud, api-backend/,
      )
      await assert.rejects(readdir(target), /ENOENT/)
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

  it('explains how to use an existing Ruvyxa project without changing it', async () => {
    const tempRoot = await mkdtemp(join(tmpdir(), 'ruvyxa-create-'))
    const target = join(tempRoot, 'existing-app')

    try {
      await mkdir(target)
      const packagePath = join(target, 'package.json')
      const originalPackage = JSON.stringify({ dependencies: { ruvyxa: '^1.0.14' } })
      await writeFile(packagePath, originalPackage)

      await assert.rejects(
        createRuvyxaApp(target),
        /An existing Ruvyxa project was detected[\s\S]*npm run dev[\s\S]*No files were changed/,
      )
      assert.equal(await readFile(packagePath, 'utf8'), originalPackage)
    } finally {
      await rm(tempRoot, { recursive: true, force: true })
    }
  })

  it('gives non-destructive guidance for a generic non-empty directory', async () => {
    const tempRoot = await mkdtemp(join(tmpdir(), 'ruvyxa-create-'))
    const target = join(tempRoot, 'notes')

    try {
      await mkdir(target)
      await writeFile(join(target, 'notes.txt'), 'keep me')
      await writeFile(join(target, 'package.json'), '{ malformed')

      await assert.rejects(
        createRuvyxaApp(target),
        /move or rename the existing directory[\s\S]*No files were changed/,
      )
    } finally {
      await rm(tempRoot, { recursive: true, force: true })
    }
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
