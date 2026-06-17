#!/usr/bin/env node
import { spawnSync } from "node:child_process"
import { chmodSync, existsSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))
const packageRoot = resolve(here, "..")
const monorepoRoot = resolve(here, "../../..")
const executable = process.platform === "win32" ? "ruvyxa.exe" : "ruvyxa"
const platformKey = `${process.platform}-${process.arch}`

const binary = findBinary()

if (!binary) {
  console.error(`Ruvyxa native CLI binary was not found for ${platformKey}.`)
  console.error("Reinstall ruvyxa, or install the matching @ruvyxa/cli-* optional package.")
  console.error("When working from source, run `cargo build -p ruvyxa_cli` first.")
  process.exit(1)
}

const result = spawnSync(binary, process.argv.slice(2), {
  cwd: process.cwd(),
  stdio: "inherit",
  shell: process.platform === "win32",
})

if (result.error) {
  console.error(`Failed to run Ruvyxa native CLI at ${binary}: ${result.error.message}`)
  process.exit(1)
}

process.exit(result.status ?? 0)

function findBinary() {
  const bundled = resolve(packageRoot, "native-bin", platformKey, executable)
  if (existsSync(bundled)) return prepareExecutable(bundled)

  const optionalPackage = optionalBinaryPackageName()
  if (optionalPackage) {
    try {
      const packageJson = import.meta.resolve(`${optionalPackage}/package.json`)
      const packageRoot = dirname(fileURLToPath(packageJson))
      const optionalBinary = join(packageRoot, "bin", executable)
      if (existsSync(optionalBinary)) return prepareExecutable(optionalBinary)
    } catch {
      // Optional platform package is absent on unsupported platforms.
    }
  }

  for (const profile of ["debug", "release"]) {
    const sourceBinary = resolve(monorepoRoot, "target", profile, executable)
    if (existsSync(sourceBinary)) return prepareExecutable(sourceBinary)
  }

  return null
}

function prepareExecutable(binary) {
  if (process.platform !== "win32") {
    chmodSync(binary, 0o755)
  }

  return binary
}

function optionalBinaryPackageName() {
  switch (platformKey) {
    case "darwin-arm64":
      return "@ruvyxa/cli-darwin-arm64"
    case "darwin-x64":
      return "@ruvyxa/cli-darwin-x64"
    case "linux-arm64":
      return "@ruvyxa/cli-linux-arm64"
    case "linux-x64":
      return "@ruvyxa/cli-linux-x64"
    case "win32-x64":
      return "@ruvyxa/cli-win32-x64"
    default:
      return null
  }
}
