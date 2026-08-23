#!/usr/bin/env node
import readline from 'node:readline'
import { STARTER_TEMPLATES, createRuvyxaApp, detectPackageManager } from '../dist/index.js'
import { createFrame } from '../dist/tty.js'

const args = process.argv.slice(2)
if (args.includes('--help') || args.includes('-h')) {
  console.log(`Usage: create-ruvyxa [directory] [--template ${STARTER_TEMPLATES.join('|')}]`)
  console.log('Run with no flags in an interactive terminal to be prompted for both.')
  process.exit(0)
}
const templateArg = args.find((arg) => arg.startsWith('--template='))
const templateIndex = args.findIndex((arg) => arg === '--template' || arg === '-t')
const templateValue = templateIndex >= 0 ? args[templateIndex + 1] : undefined
const explicitTemplate = templateArg?.slice('--template='.length) ?? templateValue
const missingTemplate =
  templateArg === '--template=' ||
  (templateIndex >= 0 && (!templateValue || templateValue.startsWith('-')))
const explicitTarget = args.find(
  (arg, index) => !arg.startsWith('-') && index !== (templateIndex >= 0 ? templateIndex + 1 : -1),
)
const color = process.stdout.isTTY && !process.env.NO_COLOR
const interactive =
  process.stdin.isTTY && process.stdout.isTTY && !explicitTemplate && !missingTemplate

// A muted dark-editor palette, used throughout: true color where the terminal advertises
// it, otherwise the nearest xterm-256 slot so it still reads the same in a 256-color one.
const truecolor = /(^|[^a-z])(truecolor|24bit)([^a-z]|$)/i.test(process.env.COLORTERM ?? '')

function ink(hex, xterm256) {
  if (!truecolor) return `38;5;${xterm256}`
  const rgb = Number.parseInt(hex, 16)
  return `38;2;${(rgb >> 16) & 255};${(rgb >> 8) & 255};${rgb & 255}`
}

const CYAN = ink('56b6c2', 73)
const GREEN = ink('98c379', 114)
const PURPLE = ink('c678dd', 176)
const RED = ink('e06c75', 174)
const COMMENT = ink('5c6370', 240)
const cyan = (value) => format(value, CYAN)
const green = (value) => format(value, GREEN)
const magenta = (value) => format(value, PURPLE)
const gray = (value) => format(value, COMMENT)
const red = (value) => format(value, RED)
const bold = (value) => format(value, '1')
const dim = (value) => format(value, '2')

function format(value, code) {
  return color ? `\x1b[${code}m${value}\x1b[0m` : value
}

if (color) {
  process.on('exit', () => process.stdout.write('\x1b[?25h'))
}

// The Ruvyxa octopus, pixel-for-pixel from examples/demo/app/components/ruvyxa-runner.tsx:
// black eyes, a purple body, four tentacles that lift left/right as it "runs" — same 4 gait
// frames as RUNNER_FRAMES in that file (idle, step-left, idle, step-right).
const RUNNER_SPRITE = [
  '00111100',
  '01111110',
  '11K11K11',
  '01111110',
  '00111100',
  '11111111',
  '10100101',
  '01011010',
]
const RUNNER_SPRITE_STEP_LEFT = [
  '00111100',
  '01111110',
  '11K11K11',
  '01111110',
  '00111100',
  '11111111',
  '10100101',
  '10010101',
]
const RUNNER_SPRITE_STEP_RIGHT = [
  '00111100',
  '01111110',
  '11K11K11',
  '01111110',
  '00111100',
  '11111111',
  '10100101',
  '10101001',
]
const RUNNER_FRAMES = [
  RUNNER_SPRITE,
  RUNNER_SPRITE_STEP_LEFT,
  RUNNER_SPRITE,
  RUNNER_SPRITE_STEP_RIGHT,
]
const MASCOT_PURPLE = 141
const MASCOT_BLACK = 16

function renderMascot(sprite) {
  if (!color) return []
  return sprite.map((row) => {
    let line = ''
    for (const cell of row) {
      if (cell === '0') {
        line += '  '
      } else {
        const bg = cell === 'K' ? MASCOT_BLACK : MASCOT_PURPLE
        line += `\x1b[48;5;${bg}m  \x1b[0m`
      }
    }
    return line
  })
}

function bannerLines(sprite, status) {
  const mascot = renderMascot(sprite)
  const info = []
  info[3] = bold(magenta('RUVYXA'))
  info[4] = dim('create-ruvyxa')
  if (!mascot.length) {
    return [`  ${bold(magenta('RUVYXA'))} ${dim('create-ruvyxa')}`, '', `  ${status}`]
  }
  const lines = mascot.map((row, i) => `  ${row}  ${info[i] ?? ''}`)
  lines.push('', `  ${status}`)
  return lines
}

// The starters do not share one layout — `blog` has routes `minimal` lacks, `api`
// has no page components at all — so the summary is rendered from the files that were
// actually written rather than from an assumed structure.
const TREE_MAX_ENTRIES = 24

// One hue per role, the way a syntax highlighter separates token kinds: red directories,
// blue markup, cyan modules, purple styles, amber config, green assets, foreground-gray
// docs, muted-gray dotfiles and branches. The project root is a directory too, so it
// reads in the same red as the rest of them.
const TREE_DIR = `1;${RED}`
const TREE_MARKUP = ink('61afef', 75)
const TREE_MODULE = CYAN
const TREE_STYLE = PURPLE
const TREE_CONFIG = ink('e5c07b', 180)
const TREE_ASSET = GREEN
const TREE_DOC = ink('abb2bf', 145)
const TREE_DOTFILE = ink('7f848e', 244)
const TREE_OTHER = ink('abb2bf', 145)
const TREE_BRANCH = COMMENT

function colorizeEntry(name, isDirectory) {
  if (isDirectory) return format(`${name}/`, TREE_DIR)
  if (/^ruvyxa\.config\.[cm]?[jt]s$/.test(name)) return bold(format(name, TREE_CONFIG))
  if (/^(package(-lock)?\.json|tsconfig(\..+)?\.json|.*\.config\.[cm]?[jt]s)$/.test(name)) {
    return format(name, TREE_CONFIG)
  }
  if (/\.[cm]?[jt]sx$/.test(name)) return format(name, TREE_MARKUP)
  if (/\.[cm]?[jt]s$/.test(name)) return format(name, TREE_MODULE)
  if (/\.(css|scss|sass|less)$/.test(name)) return format(name, TREE_STYLE)
  if (/\.(md|mdx|txt)$/.test(name)) return format(name, TREE_DOC)
  if (/\.(png|jpe?g|gif|svg|webp|avif|ico|woff2?|ttf|otf|mp4|webm)$/.test(name)) {
    return format(name, TREE_ASSET)
  }
  if (/\.(json|jsonc|ya?ml|toml)$/.test(name)) return format(name, TREE_CONFIG)
  if (name.startsWith('.')) return format(name, TREE_DOTFILE)
  return format(name, TREE_OTHER)
}

/** Build a nested tree from project-relative POSIX file paths. */
function buildTree(files) {
  const root = new Map()
  for (const file of files) {
    let node = root
    const segments = file.split('/')
    segments.forEach((segment, index) => {
      const isFile = index === segments.length - 1
      if (!node.has(segment)) node.set(segment, isFile ? null : new Map())
      if (!isFile) node = node.get(segment)
    })
  }
  return root
}

/** Directories first, then files, each alphabetically — stable across platforms. */
function sortedEntries(node) {
  return [...node.entries()].sort(([leftName, left], [rightName, right]) => {
    const leftIsDir = left !== null
    const rightIsDir = right !== null
    if (leftIsDir !== rightIsDir) return leftIsDir ? -1 : 1
    // Code units, not `localeCompare`: the claim above is that this listing is
    // the same on every platform, and locale ordering is what breaks it.
    if (leftName < rightName) return -1
    if (leftName > rightName) return 1
    return 0
  })
}

function treeLines(node, prefix = '', budget = { remaining: TREE_MAX_ENTRIES, hidden: 0 }) {
  const entries = sortedEntries(node)
  const lines = []
  for (const [index, [name, child]] of entries.entries()) {
    if (budget.remaining <= 0) {
      budget.hidden += countEntries(node, index)
      break
    }
    budget.remaining -= 1
    const isLast = index === entries.length - 1
    const connector = isLast ? '└─ ' : '├─ '
    lines.push(`${prefix}${format(connector, TREE_BRANCH)}${colorizeEntry(name, child !== null)}`)
    if (child !== null) {
      lines.push(
        ...treeLines(child, `${prefix}${format(isLast ? '   ' : '│  ', TREE_BRANCH)}`, budget),
      )
    }
  }
  return lines
}

/** Count the entries from `startIndex` onward, including everything nested below them. */
function countEntries(node, startIndex) {
  return sortedEntries(node)
    .slice(startIndex)
    .reduce((total, [, child]) => total + 1 + (child === null ? 0 : countEntries(child, 0)), 0)
}

const TEMPLATE_DESCRIPTIONS = {
  minimal: 'Blank starter — layout and a single page',
  blog: 'Content site with blog and about routes',
  crud: 'Task list backed by server actions and forms',
  api: 'API-only routes, no page UI',
}

/** Line-editing prompt (readline's own cooked-mode handling covers backspace, paste, etc). */
function promptText(question, defaultValue) {
  return new Promise((resolve) => {
    const rl = readline.createInterface({ input: process.stdin, output: process.stdout })
    const hint = defaultValue ? dim(` (${defaultValue})`) : ''
    rl.question(`  ${bold(cyan('?'))} ${question}${hint} `, (answer) => {
      rl.close()
      const value = answer.trim()
      resolve(value === '' ? defaultValue : value)
    })
    rl.on('SIGINT', () => process.exit(130))
  })
}

/** Arrow-key template picker, redrawn in place with the same cursor-save trick as the spinner. */
function selectTemplate() {
  return new Promise((resolve) => {
    const items = STARTER_TEMPLATES.map((value) => ({
      value,
      desc: TEMPLATE_DESCRIPTIONS[value] ?? '',
    }))
    let index = 0

    const renderMenu = () => [
      `  ${bold(magenta('?'))} ${bold('Select a starter template')} ${dim('(↑/↓ to move, enter to confirm)')}`,
      '',
      ...items.map((item, i) => {
        const active = i === index
        const pointer = active ? magenta('❯') : ' '
        const label = active ? bold(magenta(item.value)) : item.value
        const desc = item.desc ? dim(` — ${item.desc}`) : ''
        return `  ${pointer} ${label}${desc}`
      }),
    ]

    const frame = createFrame(process.stdout, true)
    const redraw = () => frame.render(renderMenu())

    // Wipe the whole menu with no trace left behind — the spinner/banner that follows
    // takes over the same screen space, so nothing from this prompt should linger.
    const cleanup = () => {
      process.stdin.setRawMode?.(false)
      process.stdin.removeListener('keypress', onKeypress)
      process.stdin.pause()
      frame.clear()
    }

    const onKeypress = (_str, key) => {
      if (!key) return
      if (key.name === 'up' || key.name === 'k') {
        index = (index - 1 + items.length) % items.length
        redraw()
      } else if (key.name === 'down' || key.name === 'j') {
        index = (index + 1) % items.length
        redraw()
      } else if (key.name === 'return') {
        const chosen = items[index].value
        cleanup()
        resolve(chosen)
      } else if ((key.name === 'c' && key.ctrl) || key.name === 'escape') {
        cleanup()
        process.exit(130)
      }
    }

    redraw()
    readline.emitKeypressEvents(process.stdin)
    if (process.stdin.isTTY) process.stdin.setRawMode(true)
    process.stdin.on('keypress', onKeypress)
    process.stdin.resume()
  })
}

const MASCOT_FRAME_MS = 160
const MASCOT_MIN_LOOPS = 1

function startMascotSpinner(label) {
  // One line per state, printed as it happens. Used whenever the region cannot
  // be redrawn: with no colour there is no cursor control to rely on, and a
  // banner at least as tall as the viewport has already scrolled its top row
  // out of reach, so there is nothing to move the cursor back to. Either way,
  // drawing once beats animating into the stack of copies this used to produce.
  const printOnce = () => {
    console.log(bannerLines(RUNNER_FRAMES[0], label).join('\n'))
    // The completion line is printed here too. Returning a no-op dropped it
    // silently, so a piped or redirected run reported that scaffolding had
    // started and never that it finished.
    return async (finalLabel) => {
      console.log(`  ${green('✓')} ${finalLabel ?? label}`)
    }
  }

  if (!color) return printOnce()

  const frame = createFrame(process.stdout, true)
  if (!frame.canRedraw(bannerLines(RUNNER_FRAMES[0], label))) return printOnce()

  const spinner = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏']
  let gait = 0
  let spin = 0
  const startedAt = Date.now()
  const redraw = (status) => frame.render(bannerLines(RUNNER_FRAMES[gait], status))
  redraw(`${cyan(spinner[0])} ${label}`)
  const timer = setInterval(() => {
    gait = (gait + 1) % RUNNER_FRAMES.length
    spin = (spin + 1) % spinner.length
    redraw(`${cyan(spinner[spin])} ${label}`)
  }, MASCOT_FRAME_MS)
  return async (finalLabel) => {
    const minDuration = RUNNER_FRAMES.length * MASCOT_FRAME_MS * MASCOT_MIN_LOOPS
    const elapsed = Date.now() - startedAt
    if (elapsed < minDuration) {
      await new Promise((resolve) => setTimeout(resolve, minDuration - elapsed))
    }
    clearInterval(timer)
    frame.finish(bannerLines(RUNNER_FRAMES[gait], `${green('✓')} ${finalLabel ?? label}`))
  }
}

try {
  if (missingTemplate) {
    throw new Error(
      `Starter template name is required.\n  Choose one of: ${STARTER_TEMPLATES.join(', ')}`,
    )
  }

  console.log('')
  const target =
    explicitTarget ??
    (interactive ? await promptText('Project name?', 'my-ruvyxa-app') : 'my-ruvyxa-app')
  const template = explicitTemplate ?? (interactive ? await selectTemplate() : undefined)
  if (interactive) console.log('')

  const stopSpinner = startMascotSpinner(`Scaffolding ${bold(target)}...`)
  const result = await createRuvyxaApp(target, template ? { template } : undefined)
  await stopSpinner(`Created ${bold(cyan(target))}`)

  const pm = detectPackageManager()

  console.log('')
  console.log(`  ${gray('starter:')} ${result.template}`)
  console.log('')
  const fileCount = dim(`(${result.files.length} files)`)
  console.log(`  ${bold('Project')} ${fileCount}`)
  console.log('')
  console.log(`  ${colorizeEntry(target, true)}`)
  const budget = { remaining: TREE_MAX_ENTRIES, hidden: 0 }
  for (const line of treeLines(buildTree(result.files), '  ', budget)) {
    console.log(line)
  }
  if (budget.hidden > 0) {
    console.log(`  ${dim(`… and ${budget.hidden} more`)}`)
  }
  console.log('')
  const detected = dim(`(detected: ${pm.name})`)
  console.log(`  ${bold('Next steps')} ${detected}`)
  console.log('')
  console.log(`    ${cyan('cd')} ${target}`)
  console.log(`    ${cyan(pm.install)}`)
  console.log(`    ${cyan(pm.dev)}`)
  console.log('')
  const tagline = format(
    'Clarity over cleverness. Speed by default. Control that stays yours.',
    `1;${PURPLE}`,
  )
  console.log(`  ${tagline}`)
  console.log('')
} catch (err) {
  const message = err instanceof Error ? err.message : String(err)
  console.error('')
  console.error(`  ${red('[error]')} ${message}`)
  console.error('')
  process.exit(1)
}
