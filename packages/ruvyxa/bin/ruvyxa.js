#!/usr/bin/env node
import { spawnSync } from "node:child_process"
import { existsSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = resolve(here, "../../..")
const cargoToml = resolve(repoRoot, "Cargo.toml")

if (!existsSync(cargoToml)) {
  console.error("The npm CLI wrapper currently expects to run inside the Ruvyxa monorepo.")
  process.exit(1)
}

const result = spawnSync("cargo", ["run", "-p", "ruvyxa_cli", "--", ...process.argv.slice(2)], {
  cwd: repoRoot,
  stdio: "inherit",
  shell: process.platform === "win32",
})

process.exit(result.status ?? 1)
