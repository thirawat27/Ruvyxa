#!/usr/bin/env node
import { spawnSync } from "node:child_process"
import { cp, mkdir, readFile, rm } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { currentPlatform } from "./native-platform.mjs"

const here = dirname(fileURLToPath(import.meta.url))
const ruvyxaPackageRoot = resolve(here, "..")
const repoRoot = resolve(ruvyxaPackageRoot, "../..")
const platformPackageRoot = process.cwd()
const packageJson = JSON.parse(
  await readFile(resolve(platformPackageRoot, "package.json"), "utf8"),
)
const expectedKey = packageJson.name.replace("@ruvyxa/cli-", "")
const platform = currentPlatform()

if (platform.key !== expectedKey) {
  throw new Error(
    `${packageJson.name} must be packed on ${expectedKey}, current platform is ${platform.key}`,
  )
}

const build = spawnSync("cargo", ["build", "--release", "-p", "ruvyxa_cli"], {
  cwd: repoRoot,
  stdio: "inherit",
  shell: process.platform === "win32",
})

if (build.status !== 0) {
  process.exit(build.status ?? 1)
}

await rm(resolve(platformPackageRoot, "bin"), { recursive: true, force: true })
await mkdir(resolve(platformPackageRoot, "bin"), { recursive: true })
await cp(
  resolve(repoRoot, "target", "release", platform.executable),
  resolve(platformPackageRoot, "bin", platform.executable),
)
