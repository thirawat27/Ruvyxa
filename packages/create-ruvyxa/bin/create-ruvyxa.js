#!/usr/bin/env node
import { createRuvyxaApp } from "../dist/index.js"

const target = process.argv[2] ?? "my-ruvyxa-app"

try {
  await createRuvyxaApp(target)
  console.log(`\n  Created ${target}\n`)
  console.log("  Next steps:")
  console.log(`    cd ${target}`)
  console.log("    pnpm install")
  console.log("    pnpm dev\n")
} catch (err) {
  const message = err instanceof Error ? err.message : String(err)
  console.error(`\n  Error: ${message}\n`)
  process.exit(1)
}
