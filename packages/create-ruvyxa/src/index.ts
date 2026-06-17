import { cp, mkdir } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

export async function createRuvyxaApp(targetDir: string): Promise<void> {
  const here = dirname(fileURLToPath(import.meta.url))
  const templateDir = resolve(here, "../../../templates/minimal")
  await mkdir(targetDir, { recursive: true })
  await cp(templateDir, targetDir, { recursive: true, force: false, errorOnExist: true })
}
