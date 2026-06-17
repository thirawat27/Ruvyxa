#!/usr/bin/env node
import { readdirSync, readFileSync, statSync } from "node:fs"
import { join } from "node:path"

const expectedVersion = "1.0.0"
const repoUrl = "git+https://github.com/thirawat27/ruvyxa.git"
const packageDirs = [
  "packages/ruvyxa",
  "packages/create-ruvyxa",
  ...readdirSync("packages/@ruvyxa")
    .map((name) => `packages/@ruvyxa/${name}`)
    .filter((dir) => statSync(dir).isDirectory()),
]

const failures = []

for (const dir of packageDirs) {
  const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"))
  check(pkg.version === expectedVersion, `${pkg.name} version must be ${expectedVersion}`)
  check(pkg.description?.length >= 40, `${pkg.name} needs a useful npm description`)
  check(pkg.license === "MIT", `${pkg.name} must use MIT license`)
  check(pkg.repository?.url === repoUrl, `${pkg.name} repository must point to thirawat27/ruvyxa`)
  check(pkg.bugs?.url === "https://github.com/thirawat27/ruvyxa/issues", `${pkg.name} bugs URL is invalid`)
  check(pkg.homepage === "https://github.com/thirawat27/ruvyxa#readme", `${pkg.name} homepage is invalid`)
  check(pkg.publishConfig?.access === "public", `${pkg.name} must publish with public access`)
  check(Array.isArray(pkg.files) && pkg.files.length > 0, `${pkg.name} must declare package files`)
}

if (failures.length > 0) {
  console.error(failures.map((failure) => `- ${failure}`).join("\n"))
  process.exit(1)
}

console.log(`Validated ${packageDirs.length} npm package manifests for ${expectedVersion}.`)

function check(condition, message) {
  if (!condition) failures.push(message)
}
