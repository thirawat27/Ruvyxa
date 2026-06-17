#!/usr/bin/env node
import { cp, mkdir, rm } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))
const packageRoot = resolve(here, "..")
const repoRoot = resolve(packageRoot, "../..")
const target = resolve(packageRoot, "native-src")

await rm(target, { recursive: true, force: true })
await mkdir(resolve(target, "crates"), { recursive: true })
await cp(resolve(repoRoot, "Cargo.toml"), resolve(target, "Cargo.toml"))
await cp(resolve(repoRoot, "Cargo.lock"), resolve(target, "Cargo.lock"))

for (const crate of ["ruvyxa_cli", "ruvyxa_dev_server", "ruvyxa_graph", "ruvyxa_diagnostics"]) {
  await cp(resolve(repoRoot, "crates", crate), resolve(target, "crates", crate), {
    recursive: true,
    force: true,
  })
}
