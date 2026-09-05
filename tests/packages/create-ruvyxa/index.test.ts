import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, readdir, rm, utimes, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, relative } from 'node:path'

import { repoPath } from '../../repo-root.ts'
import {
  STARTER_TEMPLATES,
  createRuvyxaApp,
  detectPackageManager,
} from '../../../packages/create-ruvyxa/dist/index.js'

const frameworkVersion = JSON.parse(await readFile(repoPath('package.json'), 'utf8'))
  .version as string

const starterScripts = {
  dev: 'ruvyxa dev',
  build: 'ruvyxa build',
  start: 'ruvyxa start',
  preview: 'ruvyxa preview',
  typecheck: 'tsc --noEmit',
  check: 'ruvyxa check',
  routes: 'ruvyxa routes',
  'routes:json': 'ruvyxa routes --json',
  analyze: 'ruvyxa analyze',
  'analyze:html': 'ruvyxa analyze --html',
  adds: 'ruvyxa adds',
  doctor: 'ruvyxa doctor',
  clean: 'ruvyxa clean',
  trace: 'ruvyxa trace',
  bench: 'ruvyxa bench',
  'test:parity': 'ruvyxa test:parity',
  plugin: 'ruvyxa plugin',
}

describe('detectPackageManager', () => {
  it("recognizes Deno's project convention", async () => {
    const root = await mkdtemp(join(tmpdir(), 'ruvyxa-deno-config-'))
    try {
      await writeFile(join(root, 'deno.json'), '{}')
      assert.deepEqual(detectPackageManager(root, {}), {
        name: 'deno',
        install: 'deno install',
        dev: 'deno task dev',
        exec: 'deno x -A npm:',
        lockfile: 'deno.lock',
      })
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

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

  it(
    'detects package managers installed as Windows command shims',
    { skip: process.platform !== 'win32' },
    async () => {
      const root = await mkdtemp(join(tmpdir(), 'ruvyxa-windows-pm-project-'))
      const bin = await mkdtemp(join(tmpdir(), 'ruvyxa-windows-pm-bin-'))
      try {
        await writeFile(join(bin, 'pnpm.cmd'), '@echo 10.0.0\r\n')
        const pathKey =
          Object.keys(process.env).find((key) => key.toLowerCase() === 'path') ?? 'PATH'
        const environment = { ...process.env, npm_config_user_agent: '', [pathKey]: bin }

        assert.equal(detectPackageManager(root, environment).name, 'pnpm')
      } finally {
        await rm(root, { recursive: true, force: true })
        await rm(bin, { recursive: true, force: true })
      }
    },
  )
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
        'README.md',
        'app/components/ruvyxa-runner.tsx',
        'app/globals.css',
        'app/layout.tsx',
        'app/page.tsx',
        'package.json',
        'postcss.config.ts',
        'public/ruvyxa.png',
        'ruvyxa.config.ts',
        'tsconfig.json',
      ])
      const packageJson = await readPackageJson(target)
      assert.equal(packageJson.name, 'my-app')
      assert.deepEqual(packageJson.scripts, starterScripts)
      assert.equal(packageJson.dependencies.ruvyxa, `^${frameworkVersion}`)
      assert.equal(packageJson.dependencies['@ruvyxa/react'], `^${frameworkVersion}`)
      // Tailwind CSS v4 ships with the starter: the config above names the
      // plugin, and these three are what the plugin chain resolves from the
      // project's own node_modules. A starter that lost one would scaffold a
      // stylesheet whose `@import 'tailwindcss'` fails the first build.
      for (const name of ['@tailwindcss/postcss', 'postcss', 'tailwindcss']) {
        assert.ok(packageJson.devDependencies[name], `the starter must declare ${name}`)
      }
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
    ['api', 'app/api/items/[id]/route.ts'],
  ] as const) {
    it(`creates the ${template} starter`, async () => {
      const tempRoot = await mkdtemp(join(tmpdir(), 'ruvyxa-create-'))
      const target = join(tempRoot, `${template}-app`)

      try {
        await createRuvyxaApp(target, { template })
        const files = await listFiles(target)
        assert.ok(files.includes(expectedFile))
        assert.ok(files.includes('.gitignore'))
        const packageJson = await readPackageJson(target)
        assert.equal(packageJson.name, `${template}-app`)
        assert.deepEqual(packageJson.scripts, starterScripts)
      } finally {
        await rm(tempRoot, { recursive: true, force: true })
      }
    })
  }

  for (const template of STARTER_TEMPLATES) {
    it(`reports the files it actually wrote for the ${template} starter`, async () => {
      const tempRoot = await mkdtemp(join(tmpdir(), 'ruvyxa-create-'))
      const target = join(tempRoot, `${template}-report`)

      try {
        const result = await createRuvyxaApp(target, { template })
        assert.equal(result.template, template)
        assert.deepEqual(result.files, await listFiles(target))
        assert.ok(result.files.includes('ruvyxa.config.ts'))
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
        /Choose one of: minimal, blog, crud, api/,
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
  dependencies: Record<string, string>
  devDependencies: Record<string, string>
}> {
  return JSON.parse(await readFile(join(root, 'package.json'), 'utf8'))
}

describe('the target path, not only its last segment', () => {
  /**
   * Every check used to read `basename(trimmed)`, so a path argument slipped
   * past all of them. `nul/my-app` has the basename `my-app` and a reserved
   * Windows name in the segment that is actually reserved; `C:\\Windows\\my-app`
   * is worse, because `basename` strips the drive letter that the
   * invalid-character check would otherwise reject.
   *
   * The checks now cover the value that gets resolved rather than describing a
   * safety they did not provide.
   */
  it('refuses a reserved or invalid name in any segment', async () => {
    for (const target of ['nul/my-app', 'foo/con/my-app', 'aux/bar/my-app']) {
      await assert.rejects(
        createRuvyxaApp(target),
        /reserved or unsafe/,
        `${target} carries a reserved Windows name in a segment that is not the last`,
      )
    }

    for (const target of ['a"b/my-app', 'a|b/my-app', 'a?b/my-app', 'a*b/my-app']) {
      await assert.rejects(
        createRuvyxaApp(target),
        /cannot contain/,
        `${target} carries a character a directory name may not have`,
      )
    }
  })

  /**
   * A path argument stays allowed. `create-ruvyxa ~/projects/foo` is a
   * reasonable thing to type, this is not a trust boundary, and refusing it
   * would be a worse answer than the bug.
   */
  it('still allows an ordinary relative path', async () => {
    const root = await mkdtemp(join(tmpdir(), 'ruvyxa-create-path-'))
    try {
      const nested = join(root, 'nested', 'my-app')
      await mkdir(join(root, 'nested'), { recursive: true })
      await createRuvyxaApp(nested)
      const entries = await readdir(nested)
      assert.ok(entries.includes('package.json'), `scaffolded into ${nested}: ${entries}`)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
})
