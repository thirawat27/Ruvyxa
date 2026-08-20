/**
 * Prove that building the same project twice produces the same bytes.
 *
 * Ruvyxa enforces the ingredients of a reproducible build in several places —
 * `localeCompare` and host-locale case folding are banned outright, ordering
 * goes through explicit comparators, and the Rust and JavaScript graphs are
 * held to shared conformance fixtures. Those are rules about the source. This
 * checks the property they exist to produce, which is the only thing a user
 * actually cares about: build, wipe, build again, compare every emitted file.
 *
 * Differences are classified rather than lumped together, because they do not
 * all mean the same thing:
 *
 * - **Emitted code** differing is a real defect and fails this check.
 * - **Build telemetry** (`cacheHits`, `durationMs`, and the rest of the fields
 *   `ruvyxa bench` reads) describes how the build *ran*. It varies with cache
 *   state and scheduling by design, and it currently sits inside
 *   `client/manifest.json` alongside deployment data.
 * - **Prerendered HTML** differing usually means the page itself renders a
 *   clock or a random value. That is the application's nondeterminism, not the
 *   framework's, so it is reported without failing.
 * - The **build cache** under `.ruvyxa/cache/` is not output at all and is
 *   skipped.
 *
 * Usage:
 *   node scripts/verify-reproducible.mjs [--root <project>] [--keep] [--strict]
 *
 * `--strict` also fails on telemetry and prerender differences.
 */
import { createHash } from 'node:crypto'
import { readdirSync, readFileSync, rmSync, statSync } from 'node:fs'
import path from 'node:path'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

/**
 * Fields `ruvyxa bench` reads out of the client manifest.
 *
 * Kept in step with `TELEMETRY_FIELDS` in `crates/ruvyxa_cli/src/bench.rs`.
 */
const TELEMETRY_FIELDS = new Set([
  'artifactCacheHit',
  'cacheHit',
  'cacheHits',
  'createdAtUnix',
  'durationMs',
  'graphHits',
  'hits',
  'misses',
  'parallelism',
])

/**
 * Whether a field records how the build ran rather than what it produced.
 *
 * Every `*Ms` key is a duration, so they are matched by shape instead of being
 * listed one by one — `build.json` carries a whole `timing` object of them and
 * a new phase should not silently start failing this check.
 */
function isTelemetryField(key) {
  return TELEMETRY_FIELDS.has(key) || key.endsWith('Ms')
}

/** Emitted trees that are build state rather than deployable output. */
const SKIPPED_DIRECTORIES = ['cache/']

function parseArgs(argv) {
  const options = { root: 'examples/demo', keep: false, strict: false }
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === '--root') {
      const value = argv[index + 1]
      if (!value) throw new Error('--root needs a project directory')
      options.root = value
      index += 1
    } else if (argument === '--keep') {
      options.keep = true
    } else if (argument === '--strict') {
      options.strict = true
    } else {
      throw new Error(`unknown argument ${JSON.stringify(argument)}`)
    }
  }
  return options
}

/** Every deployable file under `directory`, as relative POSIX paths, sorted. */
function walk(directory, base = directory) {
  const found = []
  let entries
  try {
    entries = readdirSync(directory, { withFileTypes: true })
  } catch {
    return found
  }
  // Sorted so the comparison order is itself deterministic — a check for
  // reproducibility that iterated in file-system order would be reporting on
  // its own nondeterminism as well as the build's.
  for (const entry of [...entries].sort((left, right) => (left.name < right.name ? -1 : 1))) {
    const absolute = path.join(directory, entry.name)
    const relative = path.relative(base, absolute).split(path.sep).join('/')
    if (SKIPPED_DIRECTORIES.some((skipped) => `${relative}/`.startsWith(skipped))) continue
    if (entry.isDirectory()) found.push(...walk(absolute, base))
    else if (entry.isFile()) found.push(relative)
  }
  return found
}

function fingerprint(outDir) {
  const digests = new Map()
  for (const relative of walk(outDir)) {
    const bytes = readFileSync(path.join(outDir, relative))
    digests.set(relative, createHash('sha256').update(bytes).digest('hex'))
  }
  return digests
}

/** Recursively drop every known telemetry field so the rest can be compared. */
function withoutTelemetry(value) {
  if (Array.isArray(value)) return value.map(withoutTelemetry)
  if (value && typeof value === 'object') {
    const stripped = {}
    for (const [key, nested] of Object.entries(value)) {
      if (isTelemetryField(key)) continue
      stripped[key] = withoutTelemetry(nested)
    }
    return stripped
  }
  return value
}

/**
 * Whether two copies of a JSON file agree once build telemetry is removed.
 *
 * Returns false for anything that is not JSON, or that still differs — those
 * are real artifact differences.
 */
function differsOnlyInTelemetry(file, before, after) {
  if (!file.endsWith('.json')) return false
  try {
    const a = withoutTelemetry(JSON.parse(before.toString('utf8')))
    const b = withoutTelemetry(JSON.parse(after.toString('utf8')))
    return JSON.stringify(a) === JSON.stringify(b)
  } catch {
    return false
  }
}

function build(projectRoot, label) {
  process.stdout.write(`  ${label} build...`)
  const started = Date.now()
  const result = spawnSync(
    process.platform === 'win32' ? 'cargo.exe' : 'cargo',
    ['run', '-q', '-p', 'ruvyxa_cli', '--', 'build', '--root', projectRoot],
    { cwd: repoRoot, stdio: 'pipe', encoding: 'utf8' },
  )
  if (result.status !== 0) {
    process.stdout.write(' failed\n')
    process.stderr.write(result.stdout ?? '')
    process.stderr.write(result.stderr ?? '')
    throw new Error(`${label} build exited with ${result.status}`)
  }
  process.stdout.write(` ok (${((Date.now() - started) / 1000).toFixed(1)}s)\n`)
}

/** Copy one build's files aside so the second build can be compared against them. */
function snapshot(outDir, into) {
  const files = new Map()
  for (const relative of walk(outDir)) {
    files.set(relative, readFileSync(path.join(outDir, relative)))
  }
  into.files = files
  return into
}

function main() {
  const options = parseArgs(process.argv.slice(2))
  const projectRoot = path.resolve(repoRoot, options.root)
  const outDir = path.join(projectRoot, '.ruvyxa')

  try {
    statSync(projectRoot)
  } catch {
    throw new Error(`no such project directory: ${options.root}`)
  }

  console.log(`Reproducible build check for ${options.root}`)

  rmSync(outDir, { recursive: true, force: true })
  build(options.root, 'first ')
  const first = fingerprint(outDir)
  if (first.size === 0) throw new Error(`the build wrote nothing to ${outDir}`)
  const firstFiles = snapshot(outDir, {}).files

  rmSync(outDir, { recursive: true, force: true })
  build(options.root, 'second')
  const second = fingerprint(outDir)

  const onlyFirst = [...first.keys()].filter((file) => !second.has(file))
  const onlySecond = [...second.keys()].filter((file) => !first.has(file))
  const changed = [...first.keys()].filter(
    (file) => second.has(file) && first.get(file) !== second.get(file),
  )

  const artifacts = []
  const telemetry = []
  const prerendered = []
  for (const file of changed) {
    if (file.startsWith('prerender/')) {
      prerendered.push(file)
      continue
    }
    // The second build overwrote the directory, so the first build's bytes are
    // compared from the copy held in memory.
    const onlyTelemetry = differsOnlyInTelemetry(
      file,
      firstFiles.get(file),
      readFileSync(path.join(outDir, file)),
    )
    if (onlyTelemetry) telemetry.push(file)
    else artifacts.push(file)
  }

  const report = (label, files) => {
    if (files.length === 0) return
    console.log(`\n${label}`)
    for (const file of files) console.log(`  ${file}`)
  }

  console.log(`\nCompared ${first.size} emitted files (the build cache is not output).`)
  report('Build telemetry only — how the build ran, not what it produced:', telemetry)
  report(
    'Prerendered HTML — check whether the page itself renders a clock or a random value:',
    prerendered,
  )
  report('Present in only one build:', [...onlyFirst, ...onlySecond])
  report('NOT REPRODUCIBLE — emitted code differs between builds:', artifacts)

  const fatal =
    artifacts.length +
    onlyFirst.length +
    onlySecond.length +
    (options.strict ? telemetry.length + prerendered.length : 0)

  if (fatal === 0) {
    console.log('\nEvery emitted code artifact is byte-identical across both builds.')
    if (!options.keep) rmSync(outDir, { recursive: true, force: true })
    return
  }
  console.error(
    `\n${fatal} emitted file(s) are not reproducible. Something in the build depends on ` +
      'wall-clock time, iteration order, a random value, an absolute path, or the host locale.',
  )
  process.exitCode = 1
}

try {
  main()
} catch (error) {
  console.error(`verify-reproducible: ${error instanceof Error ? error.message : error}`)
  process.exitCode = 1
}
