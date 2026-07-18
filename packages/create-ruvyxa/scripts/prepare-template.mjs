#!/usr/bin/env node
import { cp, mkdir, rename, rm } from 'node:fs/promises'
import { basename, dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const packageRoot = resolve(here, '..')
const repoRoot = resolve(packageRoot, '../..')
const templates = ['minimal', 'blog', 'crud', 'api-backend']

await rm(resolve(packageRoot, 'template'), { recursive: true, force: true })
await mkdir(resolve(packageRoot, 'template'), { recursive: true })
for (const template of templates) {
  const target = resolve(packageRoot, 'template', template)
  await cp(resolve(repoRoot, 'templates', template), target, {
    recursive: true,
    force: true,
    filter: (source) => !source.includes('node_modules'),
  })

  // npm excludes nested `.gitignore` files from package tarballs. Store the template
  // under a normal name and restore the dotfile while scaffolding a new application.
  const templateIgnore = resolve(target, '.gitignore')
  await rename(templateIgnore, resolve(target, basename(templateIgnore).slice(1)))
}
