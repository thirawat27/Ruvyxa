#!/usr/bin/env node
import { cp, mkdir, rename, rm } from 'node:fs/promises'
import { basename, dirname, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const packageRoot = resolve(here, '..')
const repoRoot = resolve(packageRoot, '../..')
const templates = ['minimal', 'blog', 'crud', 'api']
const excludedDirectories = new Set(['.ruvyxa', 'dist', 'node_modules'])

await rm(resolve(packageRoot, 'template'), { recursive: true, force: true })
await mkdir(resolve(packageRoot, 'template'), { recursive: true })
for (const template of templates) {
  const sourceRoot = resolve(repoRoot, 'templates', template)
  const target = resolve(packageRoot, 'template', template)
  await cp(sourceRoot, target, {
    recursive: true,
    force: true,
    filter: (source) => {
      const path = relative(sourceRoot, source)
      if (path === '') return true
      const [topLevel] = path.split(/[\\/]/)
      return !excludedDirectories.has(topLevel)
    },
  })

  // npm excludes nested `.gitignore` files from package tarballs. Store the template
  // under a normal name and restore the dotfile while scaffolding a new application.
  const templateIgnore = resolve(target, '.gitignore')
  await rename(templateIgnore, resolve(target, basename(templateIgnore).slice(1)))
}
