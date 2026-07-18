#!/usr/bin/env node
import { createRuvyxaApp, detectPackageManager } from '../dist/index.js'

const args = process.argv.slice(2)
if (args.includes('--help') || args.includes('-h')) {
  console.log('Usage: create-ruvyxa [directory] [--template minimal|blog|crud|api-backend]')
  process.exit(0)
}
const templateArg = args.find((arg) => arg.startsWith('--template='))
const templateIndex = args.findIndex((arg) => arg === '--template' || arg === '-t')
const templateValue = templateIndex >= 0 ? args[templateIndex + 1] : undefined
const template = templateArg?.slice('--template='.length) ?? templateValue
const missingTemplate =
  templateArg === '--template=' ||
  (templateIndex >= 0 && (!templateValue || templateValue.startsWith('-')))
const target =
  args.find(
    (arg, index) => !arg.startsWith('-') && index !== (templateIndex >= 0 ? templateIndex + 1 : -1),
  ) ?? 'my-ruvyxa-app'
const color = process.stdout.isTTY && !process.env.NO_COLOR
const cyan = (value) => format(value, '36')
const green = (value) => format(value, '32')
const gray = (value) => format(value, '90')
const red = (value) => format(value, '31')
const bold = (value) => format(value, '1')
const dim = (value) => format(value, '2')

function format(value, code) {
  return color ? `\x1b[${code}m${value}\x1b[0m` : value
}

try {
  if (missingTemplate) {
    throw new Error(
      'Starter template name is required.\n' + '  Choose one of: minimal, blog, crud, api-backend',
    )
  }
  await createRuvyxaApp(target, template ? { template } : undefined)

  const pm = detectPackageManager()

  console.log('')
  console.log(`  ${green('[ok]')} ${bold('Created')} ${cyan(target)}`)
  console.log(`  ${gray('starter:')} ${template ?? 'minimal'}`)
  console.log('')
  console.log(`  ${bold('Project')}`)
  console.log(`    ${gray('app/')}page.tsx`)
  console.log(`    ${gray('app/')}layout.tsx`)
  console.log(`    ${gray('app/')}globals.css`)
  console.log(`    ruvyxa.config.ts`)
  console.log(`    AGENTS.md`)
  console.log(`    CLAUDE.md`)
  console.log('')
  console.log(`  ${bold('Next steps')} ${dim(`(detected: ${pm.name})`)}`)
  console.log(`    cd ${target}`)
  console.log(`    ${pm.install}`)
  console.log(`    ${pm.dev}`)
  console.log('')
} catch (err) {
  const message = err instanceof Error ? err.message : String(err)
  console.error('')
  console.error(`  ${red('[error]')} ${message}`)
  console.error('')
  process.exit(1)
}
