import { readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const contractPath = path.join(root, 'tests', 'fixtures', 'adapter-contract.json')
const write = process.argv.includes('--write')
const START = '<!-- adapter-matrix:start -->'
const END = '<!-- adapter-matrix:end -->'
const CAPABILITIES = new Set(['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])

const contract = JSON.parse(await readFile(contractPath, 'utf8'))
if (contract.contract !== 'ruvyxa.adapter' || contract.schemaVersion !== 1) {
  fail('unsupported adapter contract identity or schema version')
}
if (!Array.isArray(contract.adapters) || contract.adapters.length === 0) {
  fail('adapter contract must declare at least one adapter')
}

const names = new Set()
for (const adapter of contract.adapters) {
  if (!adapter || typeof adapter !== 'object' || typeof adapter.name !== 'string') {
    fail('every adapter must have a string name')
  }
  if (names.has(adapter.name)) fail(`duplicate adapter: ${adapter.name}`)
  names.add(adapter.name)
  if (
    !Array.isArray(adapter.supports) ||
    adapter.supports.some((item) => !CAPABILITIES.has(item))
  ) {
    fail(`adapter ${adapter.name} has an invalid capability list`)
  }
  // Required rather than defaulted, so a new adapter has to decide. Absent, it
  // would read as `false` — which is the answer that was wrong for two years of
  // adapters that do serve `/__ruvyxa/image`, and nothing would have asked.
  if (typeof adapter.onDemandImages !== 'boolean') {
    fail(`adapter ${adapter.name} must declare onDemandImages as a boolean`)
  }
}

const documents = [
  {
    file: 'docs/en/20-platform-adapter-guide.md',
    headers: ['Adapter', 'Target', 'Runtime', 'Supported routes', 'On-demand images'],
    yes: 'yes',
    no: 'no',
  },
  {
    file: 'docs/th/20-platform-adapter-guide.md',
    headers: ['Adapter', 'Target', 'Runtime', 'Route ที่รองรับ', 'On-demand image'],
    yes: 'ได้',
    no: 'ไม่ได้',
  },
]

for (const document of documents) {
  const file = path.join(root, document.file)
  const source = await readFile(file, 'utf8')
  const rows = contract.adapters.map(({ name, target, runtime, supports, onDemandImages }) => [
    displayName(name),
    target,
    runtime,
    supports.map((item) => item.toUpperCase()).join(', '),
    onDemandImages ? document.yes : document.no,
  ])
  const block = [START, '', markdownTable(document.headers, rows), '', END].join('\n')
  const updated = replaceBlock(source, block, document.file)
  if (source === updated) continue
  if (!write) fail(`${document.file} is stale; run node scripts/sync-adapters.mjs --write`)
  await writeFile(file, updated)
  console.log(`updated ${document.file}`)
}

function replaceBlock(source, block, file) {
  const start = source.indexOf(START)
  const end = source.indexOf(END)
  if (start === -1 || end === -1 || end < start) fail(`${file} is missing adapter matrix markers`)
  return source.slice(0, start) + block + source.slice(end + END.length)
}

function displayName(name) {
  return name === 'aws' ? 'AWS' : name[0].toUpperCase() + name.slice(1)
}

function markdownTable(headers, rows) {
  const widths = headers.map((header, index) =>
    Math.max(header.length, 3, ...rows.map((row) => row[index].length)),
  )
  const line = (cells) =>
    `| ${cells.map((cell, index) => cell.padEnd(widths[index])).join(' | ')} |`
  return [line(headers), line(widths.map((width) => '-'.repeat(width))), ...rows.map(line)].join(
    '\n',
  )
}

function fail(message) {
  console.error(`Adapter matrix: ${message}`)
  process.exit(1)
}
