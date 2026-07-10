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
