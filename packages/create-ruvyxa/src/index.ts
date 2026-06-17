import { cp, mkdir } from "node:fs/promises"
import { existsSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

export async function createRuvyxaApp(targetDir: string): Promise<void> {
  const here = dirname(fileURLToPath(import.meta.url))
  const packagedTemplateDir = resolve(here, "../template/minimal")
  const monorepoTemplateDir = resolve(here, "../../../templates/minimal")
  const templateDir = existsSync(packagedTemplateDir) ? packagedTemplateDir : monorepoTemplateDir

  await mkdir(targetDir, { recursive: true })
  await cp(templateDir, targetDir, { recursive: true, force: false, errorOnExist: true })
}
