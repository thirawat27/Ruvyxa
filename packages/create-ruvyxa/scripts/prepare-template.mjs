#!/usr/bin/env node
import { cp, mkdir, rm } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
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
