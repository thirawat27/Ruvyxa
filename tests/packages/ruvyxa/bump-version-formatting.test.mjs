/**
 * `pnpm release:bump` has to leave a tree that passes `pnpm format:check`.
 *
 * The bump rewrites ~20 manifests with `JSON.stringify(value, null, 2)`, which
 * is not the formatting this repository keeps: Prettier collapses an array that
 * fits inside `printWidth: 100` and `JSON.stringify` never does. `format:check`
 * is a CI step on all five platforms and a `verify-release` step, so a bump left
 * the release failing a gate — and the failure named every manifest while none
 * of them said "bump".
 *
 * Two things are asserted, because either alone passes against the defect. The
 * first shows the hazard is real in this tree rather than in principle. The
 * second is why a reformat step merely existing is not enough: it has to cover
 * every write, and a `writeFileSync` added later is exactly how that stops being
 * true.
 */

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { workspacePackageDirs } from '../../../scripts/workspace-packages.mjs'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const script = path.join(repoRoot, 'scripts/bump-version.mjs')

describe('the version bump leaves manifests formatted', () => {
  /**
   * The manifests on disk are Prettier's own output — `pnpm format:check` is a
   * gate, so a file that disagreed with Prettier would already be failing CI.
   * That makes the committed bytes a free oracle: anywhere they differ from
   * `JSON.stringify(parsed, null, 2)`, a bump writing the latter breaks the
   * gate. No `prettier` process needed, which matters because spawning one per
   * manifest took forty seconds.
   */
  it('rewrites at least one manifest that JSON.stringify would format differently', () => {
    // `workspacePackageDirs` reads relative to the working directory, which is
    // the repository root for every runner in this suite.
    const { dirs } = workspacePackageDirs()
    const manifests = [
      path.join(repoRoot, 'package.json'),
      ...dirs.map((dir) => path.join(repoRoot, dir, 'package.json')),
    ]

    const divergent = manifests.filter((file) => {
      let onDisk
      try {
        onDisk = readFileSync(file, 'utf8')
      } catch {
        return false
      }
      return `${JSON.stringify(JSON.parse(onDisk), null, 2)}\n` !== onDisk
    })

    assert.ok(
      divergent.length > 0,
      'no manifest in this tree formats differently under the two writers, so this test is ' +
        'asserting nothing — check whether the reformat step in bump-version.mjs is still needed',
    )
  })

  it('routes every manifest write through the recorded-and-formatted helper', () => {
    const source = readFileSync(script, 'utf8')

    // A manifest write is a `writeFileSync` of `JSON.stringify` output. The one
    // that remains is inside `writeManifest` itself; the script's other
    // `writeFileSync` writes a `Cargo.toml` as text, which Prettier does not
    // format and which is therefore not at stake here.
    const jsonWrites = source.match(/writeFileSync\([^)]*JSON\.stringify/g) ?? []
    assert.equal(
      jsonWrites.length,
      1,
      'a manifest written outside `writeManifest` is one the reformat never sees',
    )

    // The definition plus at least two call sites, so the helper is genuinely
    // the path and not a leftover.
    const helperMentions = source.match(/writeManifest\(/g) ?? []
    assert.ok(
      helperMentions.length > 2,
      `the helper has to actually be used; saw ${helperMentions.length} mention(s)`,
    )

    assert.match(
      source,
      /'--write', \.\.\.rewritten/,
      'the bump has to reformat what it wrote, or the release fails `format:check`',
    )

    // Argv, never a command string. The file list is built from workspace
    // directory names read off disk, so interpolating it into a shell command
    // let a directory containing a quote, a space, or a `$` decide what the
    // bump executed. `execFileSync` with an argument array cannot be reached
    // that way; `execSync` always can, which is why its absence is the check.
    assert.ok(
      !/\bexecSync\(/.test(source),
      'the bump must not build a shell command string from paths it read off disk',
    )
  })
})
