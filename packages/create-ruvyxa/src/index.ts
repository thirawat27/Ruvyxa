import { cp, readdir } from "node:fs/promises"
import { existsSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

export async function createRuvyxaApp(targetDir: string): Promise<void> {
  const here = dirname(fileURLToPath(import.meta.url))
  const packagedTemplateDir = resolve(here, "../template/minimal")
  const monorepoTemplateDir = resolve(here, "../../../templates/minimal")
  const templateDir = existsSync(packagedTemplateDir) ? packagedTemplateDir : monorepoTemplateDir

  if (existsSync(targetDir)) {
    const entries = await readdir(targetDir)
    if (entries.length > 0) {
      throw new Error(
        `Directory "${targetDir}" already exists and is not empty. Please choose a different name or remove it first.`
      )
    }
  }

  await cp(templateDir, targetDir, { recursive: true })
}
