#!/usr/bin/env node
import { createRuvyxaApp } from "../dist/index.js"

const target = process.argv[2] ?? "my-ruvyxa-app"
const color = process.stdout.isTTY && !process.env.NO_COLOR
const cyan = (value) => format(value, "36")
const green = (value) => format(value, "32")
const gray = (value) => format(value, "90")
const red = (value) => format(value, "31")
const bold = (value) => format(value, "1")

function format(value, code) {
  return color ? `\x1b[${code}m${value}\x1b[0m` : value
}

try {
  await createRuvyxaApp(target)
  console.log("")
  console.log(`  ${green("[ok]")} ${bold("Created")} ${cyan(target)}`)
  console.log("")
  console.log(`  ${bold("Project")}`)
  console.log(`    ${gray("app/")}page.tsx`)
  console.log(`    ${gray("app/")}layout.tsx`)
  console.log(`    ${gray("app/api/health/")}route.ts`)
  console.log(`    ruvyxa.config.ts`)
  console.log(`    AGENTS.md`)
  console.log("")
  console.log(`  ${bold("Next steps")}`)
  console.log(`    cd ${target}`)
  console.log("    pnpm install")
  console.log("    pnpm dev")
  console.log("")
} catch (err) {
  const message = err instanceof Error ? err.message : String(err)
  console.error("")
  console.error(`  ${red("[error]")} ${message}`)
  console.error("")
  process.exit(1)
}
