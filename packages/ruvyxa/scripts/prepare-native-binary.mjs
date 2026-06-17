#!/usr/bin/env node
import { spawnSync } from "node:child_process"
import { cp, mkdir, rm } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { currentPlatform } from "./native-platform.mjs"

const here = dirname(fileURLToPath(import.meta.url))
const packageRoot = resolve(here, "..")
const repoRoot = resolve(packageRoot, "../..")
const platform = currentPlatform()

const build = spawnSync("cargo", ["build", "--release", "-p", "ruvyxa_cli"], {
  cwd: repoRoot,
  stdio: "inherit",
  shell: process.platform === "win32",
})

if (build.status !== 0) {
  process.exit(build.status ?? 1)
}

const source = resolve(repoRoot, "target", "release", platform.executable)
const targetDir = resolve(packageRoot, "native-bin", platform.key)

await rm(resolve(packageRoot, "native-bin"), { recursive: true, force: true })
await mkdir(targetDir, { recursive: true })
await cp(source, resolve(targetDir, platform.executable))
