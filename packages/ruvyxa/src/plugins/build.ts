import path from 'node:path'
import { createHash } from 'node:crypto'
import { readdirSync, statSync } from 'node:fs'
import { definePlugin } from '@ruvyxa/core/plugin'
import type { PluginBuildContext, PluginHeadEntry, RuvyxaPlugin } from '@ruvyxa/core/plugin'

import { compareStable, writePublicAsset, writePublicBinaryAsset } from './shared.js'

// ─── alias ────────────────────────────────────────────────────────────────────

/**
 * Resolves exact import specifiers to project files before the native
 * resolver, e.g. `alias({ '~content': 'content/index.ts' })`. Targets are
 * resolved from the project root.
 */
export function alias(map: Record<string, string>): RuvyxaPlugin {
  const entries = Object.entries(map)
  for (const [specifier, target] of entries) {
    if (specifier === '' || typeof target !== 'string' || target === '') {
      throw new TypeError('alias: every entry needs a non-empty specifier and target path')
    }
  }

  return definePlugin({
    name: 'ruvyxa:alias',
    register({ build }) {
      build.onResolve(({ id, root }) => {
        for (const [specifier, target] of entries) {
          if (id === specifier) return path.resolve(root, target)
        }
        return undefined
      })
    },
  })
}

// ─── bundleBudget ─────────────────────────────────────────────────────────────

export interface BundleBudgetOptions {
  /** Maximum size in KiB for any single client JavaScript file. */
  maxChunkKb?: number
  /** Maximum combined size in KiB of all client JavaScript files. */
  maxTotalKb?: number
}

/**
 * Fails the production build when emitted client JavaScript exceeds the
 * configured budget, so bundle regressions surface in CI instead of in
 * production. Sizes are measured on the final minified output.
 */
export function bundleBudget(options: BundleBudgetOptions): RuvyxaPlugin {
  const { maxChunkKb, maxTotalKb } = options ?? {}
  for (const [name, value] of Object.entries({ maxChunkKb, maxTotalKb })) {
    // `!(value > 0)` rather than `value <= 0`: NaN is a number and fails every
    // comparison, so the negated form rejects it and the direct one lets it through.
    if (value !== undefined && (typeof value !== 'number' || !(value > 0))) {
      throw new TypeError(`bundleBudget: ${name} must be a positive number of KiB`)
    }
  }
  if (maxChunkKb === undefined && maxTotalKb === undefined) {
    throw new TypeError('bundleBudget: set maxChunkKb and/or maxTotalKb')
  }

  return definePlugin({
    name: 'ruvyxa:bundle-budget',
    register({ build }) {
      build.onComplete((context) => {
        const clientDir = path.join(context.outDir, 'client')
        const files = clientJavaScriptSizes(clientDir)
        const failures: string[] = []
        if (maxChunkKb !== undefined) {
          for (const file of files) {
            if (file.bytes > maxChunkKb * 1024) {
              failures.push(
                `${file.name} is ${formatKb(file.bytes)} KiB (chunk budget ${maxChunkKb} KiB)`,
              )
            }
          }
        }
        if (maxTotalKb !== undefined) {
          const total = files.reduce((sum, file) => sum + file.bytes, 0)
          if (total > maxTotalKb * 1024) {
            failures.push(
              `client JavaScript totals ${formatKb(total)} KiB (total budget ${maxTotalKb} KiB)`,
            )
          }
        }
        if (failures.length > 0) {
          throw new Error(`bundle budget exceeded:\n- ${failures.join('\n- ')}`)
        }
      })
    },
  })
}

function clientJavaScriptSizes(clientDir: string): Array<{ name: string; bytes: number }> {
  let entries: string[]
  try {
    entries = readdirSync(clientDir, { recursive: true }) as string[]
  } catch {
    return []
  }
  const files: Array<{ name: string; bytes: number }> = []
  for (const entry of entries) {
    const name = String(entry)
    if (!name.endsWith('.js') && !name.endsWith('.mjs')) continue
    const stats = statSync(path.join(clientDir, name))
    if (stats.isFile()) files.push({ name: name.replaceAll('\\', '/'), bytes: stats.size })
  }
  return files.sort((a, b) => compareStable(a.name, b.name))
}

function formatKb(bytes: number): string {
  return (bytes / 1024).toFixed(1)
}

// ─── requireEnv ───────────────────────────────────────────────────────────────

/**
 * Fails the production build when required environment variables are missing
 * or empty, so misconfigured deployments are caught at build time.
 */
export function requireEnv(names: string[]): RuvyxaPlugin {
  if (!Array.isArray(names) || names.length === 0 || names.some((name) => !name)) {
    throw new TypeError('requireEnv: pass a non-empty array of variable names')
  }

  return definePlugin({
    name: 'ruvyxa:require-env',
    register({ build }) {
      build.onComplete(() => {
        const missing = names.filter((name) => {
          const value = process.env[name]
          return value === undefined || value === ''
        })
        if (missing.length > 0) {
          throw new Error(`missing required environment variables: ${missing.join(', ')}`)
        }
      })
    },
  })
}

// ─── fonts ────────────────────────────────────────────────────────────────────

export interface FontsOptions {
  /**
   * Google Fonts CSS URLs, exactly as they appear in a `<link rel="stylesheet">`.
   *
   * ```ts
   * fonts({ google: ['https://fonts.googleapis.com/css2?family=Inter:wght@400;700&display=swap'] })
   * ```
   */
  google: string[]
  /** Public directory the font files and stylesheet are written to. @default "/fonts" */
  publicPath?: string
  /**
   * Emit `<link rel="preload">` for every downloaded font file.
   *
   * Correct for the one or two families a page actually renders in; with many
   * families it costs more than it saves. @default true
   */
  preload?: boolean
}

/**
 * Self-hosts Google Fonts at build time.
 *
 * A `<link>` to `fonts.googleapis.com` is a render-blocking request to a third
 * party: the browser cannot paint text until it has resolved a new origin,
 * fetched the stylesheet, and then fetched the font files it names. This plugin
 * downloads the stylesheet and the `.woff2` files it references during the
 * build, rewrites the `src` URLs to local paths, and declares the resulting
 * stylesheet in `<head>` — the same fonts with no third-party origin on the
 * critical path.
 *
 * Remove the original `<link rel="stylesheet" href="https://fonts.googleapis.com/...">`
 * from your layout when you adopt this; leaving it in keeps the blocking
 * request the plugin exists to remove.
 *
 * The build needs network access. A failure is reported as a warning and the
 * build continues — a deploy should not be lost to a fetch — and an empty
 * stylesheet is written in place of the real one so the pages still ship
 * without font faces rather than with a broken reference.
 *
 * The stub is not cosmetic. `head` is fixed when the plugin is constructed, so
 * the `<link rel="stylesheet">` is in every document whether or not the
 * download succeeded; leaving the file absent pointed a render-blocking
 * request at a 404 on every page load, which is a worse version of the
 * third-party round trip this plugin exists to remove.
 */
export function fonts(options: FontsOptions): RuvyxaPlugin {
  const urls = options?.google
  if (!Array.isArray(urls) || urls.length === 0 || urls.some((url) => typeof url !== 'string')) {
    throw new TypeError('fonts: google must be a non-empty array of stylesheet URLs')
  }
  for (const url of urls) {
    if (!url.startsWith('https://fonts.googleapis.com/')) {
      throw new TypeError(`fonts: ${url} is not a fonts.googleapis.com stylesheet URL`)
    }
  }
  const publicPath = normalizeFontPublicPath(options.publicPath ?? '/fonts')
  const preload = options.preload !== false
  const stylesheetPath = `${publicPath}/fonts.css`

  // Preload hints must be declared before the build runs, so they are derived
  // from the requested families rather than from the downloaded file list. A
  // stylesheet `<link>` is enough on its own; `preload` only moves the font
  // fetch earlier by one round trip.
  const head: PluginHeadEntry[] = [
    { tag: 'link', attrs: { rel: 'stylesheet', href: stylesheetPath } },
  ]
  if (preload) {
    head.unshift({
      tag: 'link',
      attrs: { rel: 'preload', as: 'style', href: stylesheetPath },
    })
  }

  return definePlugin({
    name: 'ruvyxa:fonts',
    head,
    register({ build, diagnostics }) {
      build.onComplete(async (context) => {
        try {
          const sheets: string[] = []
          for (const url of urls) {
            // The browser user-agent decides which format Google serves; asking
            // as a modern browser gets woff2, which every supported target reads.
            const response = await fetch(url, {
              headers: {
                'user-agent':
                  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36',
              },
            })
            if (!response.ok) {
              throw new Error(`${url} responded ${response.status}`)
            }
            sheets.push(await downloadFontFiles(await response.text(), context, publicPath))
          }
          writePublicAsset(context, stylesheetPath.slice(1), sheets.join('\n'))
        } catch (error) {
          diagnostics.report({
            level: 'warning',
            code: 'RUV2103',
            message: `fonts: could not self-host Google Fonts (${
              error instanceof Error ? error.message : String(error)
            }). An empty stylesheet was written in its place; pages render with fallback fonts.`,
          })
          try {
            writePublicAsset(context, stylesheetPath.slice(1), FONTS_FALLBACK_STYLESHEET)
          } catch {
            // The warning above already names the problem. Failing here would
            // turn a missing font into a failed build, which is the trade this
            // whole handler exists to avoid.
          }
        }
      })
    },
  })
}

/**
 * Written when the download fails, so the `<link>` this plugin always emits
 * resolves instead of 404ing. Valid, empty CSS: the page falls back to the
 * font stack its own styles declare.
 */
const FONTS_FALLBACK_STYLESHEET =
  '/* ruvyxa:fonts — Google Fonts could not be downloaded during this build. */\n'

/** Download every font file a stylesheet references and rewrite its URLs. */
async function downloadFontFiles(
  css: string,
  context: PluginBuildContext,
  publicPath: string,
): Promise<string> {
  const remote = [...css.matchAll(/url\((https:\/\/fonts\.gstatic\.com\/[^)]+)\)/g)]
  let rewritten = css
  for (const [, url] of remote) {
    const fileName = fontFileName(url)
    const response = await fetch(url)
    if (!response.ok) throw new Error(`${url} responded ${response.status}`)
    const bytes = Buffer.from(await response.arrayBuffer())
    const destination = `${publicPath}/${fileName}`
    writePublicBinaryAsset(context, destination.slice(1), bytes)
    // Replacer function: `replaceAll` reads `$&` and friends out of a
    // replacement string just as `replace` does, and `destination` carries the
    // configured `publicPath`, which is not `$`-escaped.
    rewritten = rewritten.replaceAll(url, () => destination)
  }
  return rewritten
}

/**
 * Stable file name for a gstatic font URL.
 *
 * The last path segment is unique per family/weight/subset, so it needs no
 * hashing; the hash of the full URL is appended only to keep two families that
 * happen to share a segment name apart.
 */
function fontFileName(url: string): string {
  const segment = url.split('?')[0].split('/').pop() ?? 'font.woff2'
  const safe = segment.replace(/[^A-Za-z0-9._-]/g, '-')
  const digest = createHash('sha256').update(url).digest('hex').slice(0, 8)
  const dot = safe.lastIndexOf('.')
  return dot <= 0 ? `${safe}-${digest}` : `${safe.slice(0, dot)}-${digest}${safe.slice(dot)}`
}

function normalizeFontPublicPath(value: string): string {
  const trimmed = `/${String(value).replace(/(?:^\/+)|(?:\/+$)/g, '')}`
  if (trimmed === '/' || /[?#]/.test(trimmed)) {
    throw new TypeError('fonts: publicPath must be a directory path such as "/fonts"')
  }
  return trimmed
}
