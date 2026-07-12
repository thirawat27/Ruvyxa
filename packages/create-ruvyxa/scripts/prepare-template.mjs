#!/usr/bin/env node
import { cp, mkdir, rename, rm } from 'node:fs/promises'
import { basename, dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const packageRoot = resolve(here, '..')
const repoRoot = resolve(packageRoot, '../..')
const target = resolve(packageRoot, 'template', 'minimal')

await rm(resolve(packageRoot, 'template'), { recursive: true, force: true })
await mkdir(resolve(packageRoot, 'template'), { recursive: true })
await cp(resolve(repoRoot, 'templates', 'minimal'), target, {
  recursive: true,
  force: true,
  filter: (source) => !source.includes('node_modules'),
})

// npm excludes nested `.gitignore` files from package tarballs. Store the template
// under a normal name and restore the dotfile while scaffolding a new application.
const templateIgnore = resolve(target, '.gitignore')
await rename(templateIgnore, resolve(target, basename(templateIgnore).slice(1)))
