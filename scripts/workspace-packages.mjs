import { existsSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'

/**
 * The published packages in this workspace, discovered the way pnpm discovers
 * them.
 *
 * Four scripts used to open this by hand — `validate-package-metadata`,
 * `validate-release-publish-plan`, `bump-version`, and `publish-if-new` — each
 * with the same five lines: name the two top-level packages, list
 * `packages/@ruvyxa`, and read a `package.json` out of every entry. That last
 * step is the bug. A directory under `packages/@ruvyxa` is a package only if it
 * *has* a manifest; pnpm resolves the same globs and skips the ones that do
 * not. These scripts crashed instead, with an unhandled `ENOENT` and a Node
 * stack, and the thing that triggered it was ordinary: a package removed from
 * git while its `dist/` and `node_modules/` stayed on disk, which leaves a
 * directory that `git status` cannot even show — every file in it is ignored,
 * and git has nothing to report about a directory.
 *
 * So `pnpm release:validate` failed on a working tree that was clean, with an
 * error naming a file nobody had touched.
 *
 * Skipping such a directory is safe in the other direction too. A package whose
 * manifest genuinely went missing is not silently dropped: its name stays in
 * `validate-release-publish-plan`'s hand-maintained publish order and vanishes
 * from the discovered set, which is the mismatch that check exists to report.
 *
 * The two top-level names are no longer listed either. `packages/*` filtered by
 * "has a manifest" *is* `ruvyxa` and `create-ruvyxa` — `packages/@ruvyxa` is a
 * scope directory and has none — so the list was a restatement of the rule, and
 * a fifth published package would have had to be remembered in four files.
 */
export function workspacePackageDirs() {
  const dirs = []
  const ignored = []
  for (const parent of ['packages', 'packages/@ruvyxa']) {
    const found = manifestDirs(
      parent,
      'package.json',
      // The scope directory itself, reached through the `packages` pass.
      (name) => !(parent === 'packages' && name === '@ruvyxa'),
    )
    dirs.push(...found.dirs)
    ignored.push(...found.ignored)
  }
  return { dirs, ignored }
}

/**
 * Directories under `parent` that carry `manifestName`, and those that do not.
 *
 * The same rule as `workspacePackageDirs`, for the other two trees this
 * repository walks. `crates/<name>/Cargo.toml` and `templates/<name>/package.json` were
 * still being opened with a bare `readdirSync` + `readFileSync` in the very file
 * that imports the helper written to stop that — so residue under either would
 * reproduce the incident described above, verbatim, in a different directory.
 * `bump-version.mjs` already guarded its `templates/` loop, so the two files
 * disagreed about whether it mattered.
 *
 * Residue is less likely there than under `packages/`, because nothing writes to
 * them. It is not impossible, and the cost of being wrong is a release check
 * that fails on a clean tree naming a file nobody touched.
 *
 * A directory that genuinely lost its manifest is reported rather than dropped
 * in silence, and for crates `cargo metadata --locked` in `pnpm check:cargo-lock`
 * is the gate that answers for a workspace member with no manifest.
 */
export function manifestDirs(parent, manifestName, accept = () => true) {
  const dirs = []
  const ignored = []
  if (!existsSync(parent)) return { dirs, ignored }
  for (const name of readdirSync(parent).sort()) {
    if (!accept(name)) continue
    // Forward slashes rather than `join`, so the reported path reads the same
    // on every host — these are printed in failure messages.
    const dir = `${parent}/${name}`
    if (!statSync(dir).isDirectory()) continue
    if (existsSync(join(dir, manifestName))) dirs.push(dir)
    else ignored.push(dir)
  }
  return { dirs, ignored }
}
