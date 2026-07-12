import { access, constants, cp, readFile, readdir, stat, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { dirname, resolve, basename } from 'node:path'
import { fileURLToPath } from 'node:url'

export { detectPackageManager } from './detect-pm.js'
export type { PackageManager, PackageManagerInfo } from './detect-pm.js'

/** Required files that must exist in the template for a valid scaffold. */
const REQUIRED_TEMPLATE_FILES = [
  'AGENTS.md',
  'CLAUDE.md',
  'app/page.tsx',
  'app/layout.tsx',
  'app/globals.css',
  'app/css.d.ts',
  'package.json',
  'ruvyxa.config.ts',
] as const

/** Characters that are invalid in directory names across platforms. */
const INVALID_DIR_CHARS = /[<>:"|?*\x00-\x1f]/

/** Maximum directory name length (Windows has 260 char path limit). */
const MAX_DIR_NAME_LENGTH = 128

/** Reserved Windows device names that are invalid as project directory names. */
const RESERVED_WINDOWS_NAMES = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(\..*)?$/i

/**
 * Create a new Ruvyxa application from the minimal template.
 *
 * Performs the following safety checks before scaffolding:
 * 1. Validates the target directory name for filesystem safety
 * 2. Checks write access on the parent directory
 * 3. Ensures target directory is empty if it exists
 * 4. Validates that the template contains all required files
 * 5. Copies the template with recursive directory creation
 *
 * @param targetDir - Path where the new project will be created
 * @throws Error with descriptive message on any validation failure
 */
export async function createRuvyxaApp(targetDir: string): Promise<void> {
  // --- Input Validation ---
  if (!targetDir || typeof targetDir !== 'string') {
    throw new Error('Project directory name is required.\n' + '  Usage: npx create-ruvyxa my-app')
  }

  const trimmed = targetDir.trim()
  if (trimmed === '') {
    throw new Error(
      'Project directory name must not be empty.\n' + '  Usage: npx create-ruvyxa my-app',
    )
  }

  if (trimmed !== targetDir) {
    throw new Error(
      'Project directory name must not start or end with whitespace.\n' +
        '  Try a name like: my-ruvyxa-app',
    )
  }

  const dirName = basename(trimmed)
  if (INVALID_DIR_CHARS.test(dirName)) {
    throw new Error(
      `Invalid project name "${dirName}". Directory names cannot contain: < > : " | ? *\n` +
        '  Try a name with only letters, numbers, dashes, and underscores.',
    )
  }

  if (RESERVED_WINDOWS_NAMES.test(dirName) || /[. ]$/.test(dirName)) {
    throw new Error(
      `Invalid project name "${dirName}". This name is reserved or unsafe on Windows.\n` +
        '  Try a name like: my-ruvyxa-app',
    )
  }

  if (dirName.length > MAX_DIR_NAME_LENGTH) {
    throw new Error(
      `Project name "${dirName}" is too long (${dirName.length} chars). ` +
        `Maximum is ${MAX_DIR_NAME_LENGTH} characters.`,
    )
  }

  if (dirName.startsWith('.') || dirName.startsWith('-')) {
    throw new Error(
      `Project name "${dirName}" should not start with "." or "-".\n` +
        '  Try a name like: my-ruvyxa-app',
    )
  }

  // --- Target Directory Checks ---
  const resolvedTarget = resolve(trimmed)

  if (existsSync(resolvedTarget)) {
    const stats = await stat(resolvedTarget)
    if (!stats.isDirectory()) {
      throw new Error(
        `"${trimmed}" already exists and is not a directory.\n` +
          '  Please choose a different name or remove the existing file.',
      )
    }

    const entries = await readdir(resolvedTarget)
    if (entries.length > 0) {
      throw new Error(
        `Directory "${trimmed}" already exists and is not empty.\n` +
          '  Please choose a different name or remove the existing directory.',
      )
    }
  }

  // --- Parent Directory Permissions ---
  const parentDir = dirname(resolvedTarget)
  try {
    await access(parentDir, constants.W_OK)
  } catch {
    throw new Error(
      `Cannot write to "${parentDir}". Permission denied.\n` +
        '  Check that you have write access to the parent directory.',
    )
  }

  // --- Locate Template ---
  const here = dirname(fileURLToPath(import.meta.url))
  const packagedTemplateDir = resolve(here, '../template/minimal')
  const monorepoTemplateDir = resolve(here, '../../../templates/minimal')
  const templateDir = existsSync(packagedTemplateDir) ? packagedTemplateDir : monorepoTemplateDir

  if (!existsSync(templateDir)) {
    throw new Error(
      'Template directory was not found. The create-ruvyxa package may be corrupted.\n' +
        '  Try reinstalling: npm i -g create-ruvyxa@latest',
    )
  }

  // --- Template Validation ---
  const missingFiles: string[] = []
  for (const required of REQUIRED_TEMPLATE_FILES) {
    const filePath = resolve(templateDir, required)
    if (!existsSync(filePath)) {
      missingFiles.push(required)
    }
  }

  if (missingFiles.length > 0) {
    throw new Error(
      'Template is incomplete. Missing required files:\n' +
        missingFiles.map((f) => `  - ${f}`).join('\n') +
        '\n' +
        '  The create-ruvyxa package may be corrupted. Try reinstalling.',
    )
  }

  // --- Copy Template ---
  try {
    await cp(templateDir, resolvedTarget, { recursive: true })
    await writeProjectPackageName(resolvedTarget, toPackageName(dirName))
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    throw new Error(
      `Failed to create project at "${trimmed}".\n` +
        `  ${message}\n` +
        '  Check disk space and filesystem access.',
    )
  }
}

/** Convert a filesystem project name into a portable, unscoped npm package name. */
function toPackageName(projectName: string): string {
  const packageName = projectName
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^[._-]+|[._-]+$/g, '')

  return packageName || 'ruvyxa-app'
}

/** Update only the copied template manifest so every scaffold owns its package identity. */
async function writeProjectPackageName(targetDir: string, packageName: string): Promise<void> {
  const packagePath = resolve(targetDir, 'package.json')
  const packageJson = JSON.parse(await readFile(packagePath, 'utf8'))
  packageJson.name = packageName
  await writeFile(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`)
}
