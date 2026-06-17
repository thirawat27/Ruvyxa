#!/usr/bin/env node
import { createRuvyxaApp } from "../dist/index.js"

const target = process.argv[2] ?? "my-ruvyxa-app"

await createRuvyxaApp(target)
console.log(`Created ${target}`)
