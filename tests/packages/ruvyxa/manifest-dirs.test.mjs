/**
 * Every tree this repository walks skips a directory with no manifest.
 *
 * `workspace-packages.mjs` documents the incident it was written for: a package
 * removed from git while its `dist/` and `node_modules/` stayed on disk leaves a
 * directory `git status` cannot even show, and the scripts crashed on it with an
 * unhandled `ENOENT`. `pnpm release:validate` failed on a clean working tree,
 * naming a file nobody had touched.
 *
 * The same bare `readdirSync` + `readFileSync` survived for `crates/` and
 * `templates/` in the very file that imports the helper — and `bump-version.mjs`
 * guarded its `templates/` loop, so two scripts disagreed about whether it
 * mattered. This holds the rule for all three.
 */

import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { after, describe, it } from 'node:test'

import { manifestDirs } from '../../../scripts/workspace-packages.mjs'

const root = mkdtempSync(path.join(tmpdir(), 'ruvyxa-manifest-dirs-'))
after(() => rmSync(root, { recursive: true, force: true }))

describe('manifestDirs', () => {
  it('reports a directory with no manifest instead of crashing on it', () => {
    const parent = path.join(root, 'crates')
    mkdirSync(path.join(parent, 'real'), { recursive: true })
    writeFileSync(path.join(parent, 'real', 'Cargo.toml'), '[package]\nname = "real"\n')
    // Residue: the shape that caused the incident.
    mkdirSync(path.join(parent, 'leftover', 'target'), { recursive: true })

    const { dirs, ignored } = manifestDirs(parent, 'Cargo.toml')

    assert.deepEqual(dirs, [`${parent}/real`])
    assert.deepEqual(ignored, [`${parent}/leftover`])
  })

  it('answers an absent parent with nothing rather than throwing', () => {
    assert.deepEqual(manifestDirs(path.join(root, 'never-created'), 'package.json'), {
      dirs: [],
      ignored: [],
    })
  })

  it('honours the caller filter, which is how the scope directory is skipped', () => {
    const parent = path.join(root, 'packages')
    mkdirSync(path.join(parent, 'kept'), { recursive: true })
    writeFileSync(path.join(parent, 'kept', 'package.json'), '{}')
    mkdirSync(path.join(parent, 'skipped'), { recursive: true })
    writeFileSync(path.join(parent, 'skipped', 'package.json'), '{}')

    const { dirs } = manifestDirs(parent, 'package.json', (name) => name !== 'skipped')
    assert.deepEqual(dirs, [`${parent}/kept`])
  })
})
