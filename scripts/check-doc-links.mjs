#!/usr/bin/env node
// Every relative Markdown link in the repository is a promise that a file
// exists at that path. Documentation reorganizations break those promises
// silently: the moved file is still in the tree, the old link still renders as
// a link, and nothing in `cargo`, `pnpm`, or the formatter looks at link
// targets. The 1.0.26 docs restructure left 25 dead links behind this way,
// including two in `packages/create-ruvyxa/README.md`, which npm renders on
// the package page.
//
// This check resolves every relative link and heading anchor against the real
// tree and fails the build when one does not land.
import { existsSync, readFileSync, statSync } from 'node:fs'
import { dirname, join, normalize, sep } from 'node:path'
import { execFileSync } from 'node:child_process'

// Inline links `[text](target)`, and reference definitions `[label]: target`.
const INLINE_LINK = /\[[^\]]*\]\(\s*<?([^)<>\s]+)>?(?:\s+"[^"]*")?\s*\)/g
const REFERENCE_LINK = /^\s{0,3}\[[^\]]+\]:\s*<?([^\s<>]+)>?/gm
// Fenced and indented code blocks contain example links to paths that are not
// expected to exist in this repository, so they are masked before scanning.
const FENCED_CODE = /^\s{0,3}(`{3,}|~{3,})[^\n]*\n[\s\S]*?^\s{0,3}\1[^\n]*$/gm
const INLINE_CODE = /`[^`\n]*`/g

const files = execFileSync('git', ['ls-files', '*.md'], { encoding: 'utf8' })
  .split('\n')
  .map((file) => file.trim())
  .filter((file) => file && !file.includes('node_modules/'))

const anchorCache = new Map()
const failures = []

for (const file of files) {
  const source = readFileSync(file, 'utf8')
  const scannable = source.replace(FENCED_CODE, '').replace(INLINE_CODE, '')

  for (const target of linkTargets(scannable)) {
    // External schemes, in-page anchors, and template placeholders are out of
    // scope: only repository-relative paths can be verified from the tree.
    if (/^(?:[a-z][a-z0-9+.-]*:|\/\/|#|\{|\$)/i.test(target)) continue

    const [path, anchor] = splitAnchor(target)
    if (!path) continue

    const resolved = normalize(join(dirname(file), decodeURIComponent(path)))
    if (!existsSync(resolved)) {
      failures.push(`${file} -> ${target} (no such file: ${resolved.split(sep).join('/')})`)
      continue
    }
    if (!anchor || !resolved.endsWith('.md') || statSync(resolved).isDirectory()) continue
    if (!headingAnchors(resolved).has(anchor)) {
      failures.push(`${file} -> ${target} (file exists, heading "#${anchor}" does not)`)
    }
  }
}

if (failures.length > 0) {
  console.error(`Broken Markdown links (${failures.length}):`)
  console.error(failures.map((failure) => `- ${failure}`).join('\n'))
  process.exit(1)
}

console.log(`Markdown links resolve across ${files.length} files.`)

/** Collect inline and reference-style link targets from already-masked text. */
function linkTargets(text) {
  const targets = []
  for (const pattern of [INLINE_LINK, REFERENCE_LINK]) {
    pattern.lastIndex = 0
    let match
    while ((match = pattern.exec(text))) targets.push(match[1])
  }
  return targets
}

/** Split `path#anchor`, tolerating anchors on paths that contain no `#`. */
function splitAnchor(target) {
  const hash = target.indexOf('#')
  if (hash < 0) return [target, '']
  return [target.slice(0, hash), target.slice(hash + 1)]
}

/**
 * GitHub's heading slugs: lowercase, drop everything that is not a word
 * character, space, or hyphen, then join words with hyphens. Inline formatting
 * markers are stripped first so `## \`cache()\` basics` slugs the same way
 * GitHub renders it.
 */
function headingAnchors(file) {
  const cached = anchorCache.get(file)
  if (cached) return cached

  const anchors = new Set()
  const source = readFileSync(file, 'utf8').replace(FENCED_CODE, '')
  for (const [, text] of source.matchAll(/^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$/gm)) {
    const slug = text
      .replace(/`([^`]*)`/g, '$1')
      .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
      .replace(/[*_~]/g, '')
      .toLowerCase()
      .replace(/[^\p{L}\p{N} -]/gu, '')
      .trim()
      .replace(/ +/g, '-')
    if (slug) anchors.add(slug)
  }
  anchorCache.set(file, anchors)
  return anchors
}
