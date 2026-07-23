import type { BuildContext } from './types.js'

/** Return the standard client bundle paths consumed by deployment adapters. */
export function clientBuildOutput(ctx: BuildContext): {
  clientDir: string
  chunkManifest: string
} {
  return {
    clientDir: `${ctx.outDir}/client`,
    chunkManifest: ctx.chunkManifest ?? `${ctx.outDir}/client/chunk-manifest.json`,
  }
}

/**
 * Return `ctx.outDir` as a project-root-relative POSIX path.
 *
 * Adapter-generated config files (netlify.toml, wrangler.jsonc) are read on
 * other machines and other operating systems, so they must never embed the
 * absolute build-machine path that `BuildContext.outDir` carries. Windows
 * separators are normalized to `/` and a trailing separator is dropped. When
 * `outDir` does not live under `root` (already relative, or a custom
 * out-of-tree directory), the normalized value is returned unchanged.
 */
export function projectRelativeOutDir(ctx: BuildContext): string {
  const normalize = (value: string) => value.replace(/\\/g, '/').replace(/\/+$/, '')
  const root = normalize(ctx.root)
  const outDir = normalize(ctx.outDir)
  if (root !== '' && outDir.startsWith(`${root}/`)) {
    return outDir.slice(root.length + 1)
  }
  return outDir
}

export function validateBuildContext(
  ctx: BuildContext,
  adapterName: string,
): asserts ctx is BuildContext {
  if (!ctx.root || typeof ctx.root !== 'string') {
    throw new Error(
      `[RUV2000] ${adapterName}: BuildContext.root is required and must be a non-empty string`,
    )
  }
  if (!ctx.outDir || typeof ctx.outDir !== 'string') {
    throw new Error(
      `[RUV2000] ${adapterName}: BuildContext.outDir is required and must be a non-empty string`,
    )
  }
}
