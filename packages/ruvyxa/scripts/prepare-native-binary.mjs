#!/usr/bin/env node
import { spawnSync } from "node:child_process"
import { chmod, cp, mkdir, rm } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { currentPlatform } from "./native-platform.mjs"

// When publishing the main ruvyxa package in CI, the native binary is
// delivered via the optional @ruvyxa/cli-* platform packages.  Skip the
// cargo build to avoid transient crates.io failures and unnecessary
// compilation on a platform that may not match the user's machine.
if (process.env.SKIP_NATIVE_BUILD === "1") {
  console.log("[prepare-native-binary] SKIP_NATIVE_BUILD=1 — skipping cargo build")
  process.exit(0)
}

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
const target = resolve(targetDir, platform.executable)
await cp(source, target)

if (process.platform !== "win32") {
  await chmod(target, 0o755)
}
