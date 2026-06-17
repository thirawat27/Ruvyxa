#!/usr/bin/env node
import { spawnSync } from "node:child_process"
import { existsSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))
const packageRoot = resolve(here, "..")
const sourceRoot = resolve(packageRoot, "native-src")
const monorepoRoot = resolve(here, "../../..")
const sourceCargoToml = resolve(sourceRoot, "Cargo.toml")
const monorepoCargoToml = resolve(monorepoRoot, "Cargo.toml")

const cargoRoot = existsSync(sourceCargoToml) ? sourceRoot : existsSync(monorepoCargoToml) ? monorepoRoot : null

if (!cargoRoot) {
  console.error("Ruvyxa CLI source was not found in this npm package.")
  console.error("Reinstall ruvyxa, or run from a complete Ruvyxa source checkout.")
  process.exit(1)
}

const result = spawnSync("cargo", ["run", "-p", "ruvyxa_cli", "--", ...process.argv.slice(2)], {
  cwd: cargoRoot,
  stdio: "inherit",
  shell: process.platform === "win32",
})

process.exit(result.status ?? 1)
